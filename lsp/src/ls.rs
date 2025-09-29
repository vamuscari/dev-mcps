use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::io::{BufRead, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;
use url::Url;

/// Minimal LSP client manager that speaks Content-Length framed JSON-RPC.

#[derive(Clone, Copy, Debug)]
enum Framing {
    ContentLength,
    Newline,
}

#[derive(Clone, Copy, Debug)]
enum FramingPreference {
    Auto,
    ContentLength,
    Newline,
}

impl FramingPreference {
    fn from_env() -> Self {
        match std::env::var("LSP_STDIO_FRAMING") {
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "" | "auto" => FramingPreference::Auto,
                "newline" | "line" | "lines" => FramingPreference::Newline,
                "content-length" | "content_length" | "contentlength" | "cl" => {
                    FramingPreference::ContentLength
                }
                other => {
                    eprintln!(
                        "codex-lsp: unknown LSP_STDIO_FRAMING value '{}'; falling back to auto",
                        other
                    );
                    FramingPreference::Auto
                }
            },
            Err(_) => FramingPreference::Auto,
        }
    }

    fn initial_read_mode(self) -> Option<Framing> {
        match self {
            FramingPreference::Auto => None,
            FramingPreference::ContentLength => Some(Framing::ContentLength),
            FramingPreference::Newline => Some(Framing::Newline),
        }
    }
}

pub struct LanguageServerManager {
    default_cmd: Option<String>,
    current_cmd: Option<String>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<std::io::BufReader<ChildStdout>>,
    next_id: i64,
    server_capabilities: Option<Value>,
    write_pref: FramingPreference,
    read_mode: Option<Framing>,
}

impl LanguageServerManager {
    fn client_capabilities() -> Value {
        json!({
            "workspace": {
                "configuration": true
            },
            "textDocument": {
                "hover": {
                    "contentFormat": ["markdown", "plaintext"]
                },
                "completion": {
                    "completionItem": {
                        "documentationFormat": ["markdown", "plaintext"],
                        "snippetSupport": false,
                        "resolveSupport": {
                            "properties": ["documentation", "detail", "additionalTextEdits"]
                        }
                    }
                },
                "codeAction": {
                    "codeActionLiteralSupport": {
                        "codeActionKind": {
                            "valueSet": [
                                "",
                                "quickfix",
                                "refactor",
                                "source"
                            ]
                        }
                    }
                }
            },
            "general": {
                "positionEncodings": ["utf-16"]
            }
        })
    }

    fn path_to_file_uri(path: &std::path::Path) -> Result<String> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        Url::from_file_path(&abs)
            .map(|url| url.to_string())
            .map_err(|_| anyhow!("failed to convert path {:?} to file URI", abs))
    }

    #[allow(dead_code)]
    pub fn new() -> Self {
        let default_cmd = std::env::var("LSP_SERVER_CMD").ok();
        Self {
            default_cmd,
            current_cmd: None,
            child: None,
            stdin: None,
            stdout: None,
            next_id: 1,
            server_capabilities: None,
            write_pref: FramingPreference::Auto,
            read_mode: None,
        }
    }

    pub fn with_command(cmd: String) -> Self {
        Self {
            default_cmd: Some(cmd),
            current_cmd: None,
            child: None,
            stdin: None,
            stdout: None,
            next_id: 1,
            server_capabilities: None,
            write_pref: FramingPreference::Auto,
            read_mode: None,
        }
    }

    fn command_parts(cmd: &str) -> Result<Vec<String>> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut chars = cmd.chars().peekable();
        let mut in_quotes: Option<char> = None;

        while let Some(ch) = chars.next() {
            match ch {
                '\'' | '"' => {
                    if let Some(active) = in_quotes {
                        if active == ch {
                            in_quotes = None;
                        } else {
                            current.push(ch);
                        }
                    } else {
                        in_quotes = Some(ch);
                    }
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                c if c.is_whitespace() && in_quotes.is_none() => {
                    if !current.is_empty() {
                        parts.push(current);
                        current = String::new();
                    }
                }
                c => current.push(c),
            }
        }

        if let Some(active) = in_quotes {
            return Err(anyhow!(
                "unclosed quote {} in language server command '{}'",
                active,
                cmd
            ));
        }

        if !current.is_empty() {
            parts.push(current);
        }

        if parts.is_empty() {
            return Err(anyhow!("empty language server command"));
        }

        Ok(parts)
    }

    fn current_write_mode(&self) -> Framing {
        match self.write_pref {
            FramingPreference::ContentLength => Framing::ContentLength,
            FramingPreference::Newline => Framing::Newline,
            FramingPreference::Auto => self.read_mode.unwrap_or(Framing::ContentLength),
        }
    }

    fn write_body(writer: &mut ChildStdin, body: &str, framing: Framing) -> Result<()> {
        match framing {
            Framing::ContentLength => {
                write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
                writer.write_all(body.as_bytes())?;
                writer.flush()?;
            }
            Framing::Newline => {
                writer.write_all(body.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
        Ok(())
    }

    fn write_jsonrpc(&mut self, value: &Value) -> Result<()> {
        let payload = serde_json::to_string(value)?;
        let framing = self.current_write_mode();
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("language server stdin closed"))?;
        Self::write_body(stdin, &payload, framing)
    }

    fn send_jsonrpc_response(&mut self, id: Value, result: Value) -> Result<()> {
        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        self.write_jsonrpc(&response)
    }

    fn send_jsonrpc_error(&mut self, id: Value, code: i64, message: String) -> Result<()> {
        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message,
            }
        });
        self.write_jsonrpc(&response)
    }

    fn handle_server_request(
        &mut self,
        id: Value,
        method: &str,
        params: Option<&Value>,
    ) -> Result<()> {
        match method {
            "workspace/configuration" => {
                let count = params
                    .and_then(|p| p.get("items"))
                    .and_then(|items| items.as_array())
                    .map(|items| items.len())
                    .unwrap_or(0);
                let results: Vec<Value> = vec![Value::Null; count];
                let result = Value::Array(results);
                eprintln!(
                    "codex-lsp: auto-responding to server request '{}' with default configuration",
                    method
                );
                self.send_jsonrpc_response(id, result)
            }
            "client/registerCapability" | "client/unregisterCapability" => {
                eprintln!(
                    "codex-lsp: acknowledging server request '{}' with null result",
                    method
                );
                self.send_jsonrpc_response(id, Value::Null)
            }
            "window/workDoneProgress/create" | "workspace/workDoneProgress/create" => {
                eprintln!(
                    "codex-lsp: acknowledging server request '{}' with null result",
                    method
                );
                self.send_jsonrpc_response(id, Value::Null)
            }
            "workspace/workspaceFolders" => {
                eprintln!(
                    "codex-lsp: responding to server request '{}' with no workspace folders",
                    method
                );
                self.send_jsonrpc_response(id, Value::Null)
            }
            "workspace/applyEdit" => {
                eprintln!(
                    "codex-lsp: rejecting server request '{}' (workspace edits unsupported)",
                    method
                );
                let result = json!({
                    "applied": false,
                    "failureReason": "codex-lsp bridge cannot apply workspace edits",
                });
                self.send_jsonrpc_response(id, result)
            }
            "window/showMessageRequest" => {
                if let Some(params) = params {
                    if let Some(message) = params.get("message").and_then(|m| m.as_str()) {
                        eprintln!("codex-lsp: server showMessageRequest -> {message}");
                    }
                }
                self.send_jsonrpc_response(id, Value::Null)
            }
            "workspace/codeLens/refresh"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlineValue/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/diagnostic/refresh" => {
                eprintln!(
                    "codex-lsp: acknowledging server refresh request '{}' with null result",
                    method
                );
                self.send_jsonrpc_response(id, Value::Null)
            }
            _ => {
                let message =
                    format!("codex-lsp bridge does not implement client request '{method}'");
                eprintln!(
                    "codex-lsp: replying to unsupported server request '{}' with MethodNotFound",
                    method
                );
                self.send_jsonrpc_error(id, -32601, message)
            }
        }
    }

    fn parse_content_length(line: &str) -> Option<usize> {
        line.to_ascii_lowercase()
            .strip_prefix("content-length:")
            .and_then(|rest| rest.trim().parse().ok())
    }

    fn read_content_length_message(
        r: &mut std::io::BufReader<ChildStdout>,
        first_line: Option<String>,
    ) -> Result<String> {
        let mut content_length: Option<usize> = None;
        if let Some(line) = first_line.as_ref() {
            if let Some(len) = Self::parse_content_length(line) {
                content_length = Some(len);
            }
        }

        let mut line = String::new();
        loop {
            line.clear();
            let n = r.read_line(&mut line)?;
            if n == 0 {
                return Err(anyhow!("EOF from language server"));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if content_length.is_none() {
                if let Some(len) = Self::parse_content_length(&line) {
                    content_length = Some(len);
                }
            }
        }
        let len = content_length.ok_or_else(|| anyhow!("missing Content-Length"))?;
        let mut buf = vec![0u8; len];
        r.read_exact(&mut buf)?;
        String::from_utf8(buf).context("utf8 body")
    }

    fn read_newline_message(
        r: &mut std::io::BufReader<ChildStdout>,
        first_line: Option<String>,
    ) -> Result<String> {
        if let Some(line) = first_line {
            return Ok(line.trim_end_matches(['\r', '\n']).to_string());
        }
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Err(anyhow!("EOF from language server"));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn read_detected_message(&mut self, first_line: Option<String>) -> Result<(String, Framing)> {
        if let Some(line) = first_line {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                return self.read_detected_message(None);
            }
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                return Ok((trimmed.to_string(), Framing::Newline));
            }
            let stdout = self
                .stdout
                .as_mut()
                .ok_or_else(|| anyhow!("language server stdout closed"))?;
            let body = Self::read_content_length_message(stdout, Some(line))?;
            return Ok((body, Framing::ContentLength));
        }

        let stdout = self
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("language server stdout closed"))?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = stdout.read_line(&mut line)?;
            if n == 0 {
                return Err(anyhow!("EOF from language server"));
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                return Ok((trimmed.to_string(), Framing::Newline));
            }
            let body = Self::read_content_length_message(stdout, Some(line.clone()))?;
            return Ok((body, Framing::ContentLength));
        }
    }

    fn read_message(&mut self) -> Result<Value> {
        let mode = self.read_mode;
        match mode {
            Some(Framing::ContentLength) => {
                let stdout = self
                    .stdout
                    .as_mut()
                    .ok_or_else(|| anyhow!("language server stdout closed"))?;
                let body = Self::read_content_length_message(stdout, None)?;
                serde_json::from_str(&body).context("parse lsp response")
            }
            Some(Framing::Newline) => {
                let stdout = self
                    .stdout
                    .as_mut()
                    .ok_or_else(|| anyhow!("language server stdout closed"))?;
                let body = Self::read_newline_message(stdout, None)?;
                serde_json::from_str(&body).context("parse lsp response")
            }
            None => {
                let (body, framing) = self.read_detected_message(None)?;
                self.read_mode = Some(framing);
                serde_json::from_str(&body).context("parse lsp response")
            }
        }
    }

    fn stop_child(&mut self) -> Result<()> {
        if self.child.is_some() {
            // Attempt graceful shutdown if streams are still available.
            if self.stdin.is_some() && self.stdout.is_some() {
                let shutdown = json!({
                    "jsonrpc": "2.0",
                    "id": self.alloc_id(),
                    "method": "shutdown",
                });
                let _ = self.write_jsonrpc(&shutdown);
                let _ = self.read_message();
                let exit = json!({"jsonrpc": "2.0", "method": "exit"});
                let _ = self.write_jsonrpc(&exit);
            }

            // Drop streams so EOF propagates.
            self.stdin = None;
            self.stdout = None;

            if let Some(mut child) = self.child.take() {
                // Give the server a moment to exit cleanly after the shutdown handshake.
                for _ in 0..10 {
                    match child.try_wait() {
                        Ok(Some(_status)) => break,
                        Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                        Err(e) => return Err(e.into()),
                    }
                }
                if child.try_wait()?.is_none() {
                    // Server did not exit in time; terminate forcefully.
                    match child.kill() {
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {}
                        Err(e) => return Err(e.into()),
                    }
                    let _ = child.wait();
                }
            }
        } else {
            self.stdin = None;
            self.stdout = None;
        }

        self.server_capabilities = None;
        self.next_id = 1;
        self.read_mode = self.write_pref.initial_read_mode();
        Ok(())
    }

    fn start_server(&mut self, cmd: &str) -> Result<()> {
        let parts = Self::command_parts(cmd)?;
        let mut command = Command::new(&parts[0]);
        if parts.len() > 1 {
            command.args(&parts[1..]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn lsp server '{}'", cmd))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        self.stdin = Some(stdin);
        self.stdout = Some(std::io::BufReader::new(stdout));
        self.child = Some(child);
        self.server_capabilities = None;
        self.next_id = 1;
        self.write_pref = FramingPreference::from_env();
        self.read_mode = self.write_pref.initial_read_mode();

        let init_result = (|| -> Result<()> {
            // Minimal initialize handshake. Use current working directory as the workspace root
            // so servers like rust-analyzer can locate files on disk without an explicit didOpen.
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let root_uri = Self::path_to_file_uri(&cwd)
                .with_context(|| format!("build rootUri for workspace at {:?}", cwd))?;
            let id = self.alloc_id();
            let init = json!({
                "jsonrpc":"2.0",
                "id": id,
                "method":"initialize",
                "params": {
                    "processId": null,
                    "capabilities": Self::client_capabilities(),
                    "rootUri": root_uri,
                    "workspaceFolders": [{"uri": root_uri, "name": "workspace"}]
                }
            });
            self.write_jsonrpc(&init)?;
            let init_value = loop {
                let value = self
                    .read_message()
                    .context("parse initialize response payload")?;
                if value.get("id") == Some(&json!(id)) {
                    break value;
                }
                if let Some(method_name) = value.get("method").and_then(|m| m.as_str()) {
                    // If the server sends requests (with an id) during initialization for things
                    // like progress or configuration, handle them to avoid deadlocks.
                    if let Some(req_id) = value.get("id").cloned() {
                        if let Err(err) =
                            self.handle_server_request(req_id, method_name, value.get("params"))
                        {
                            eprintln!(
                                "codex-lsp: failed to handle server request '{}' during initialize: {err:#}",
                                method_name
                            );
                        }
                        continue;
                    }
                    eprintln!(
                        "codex-lsp: dropping notification '{}' received during initialize",
                        method_name
                    );
                } else {
                    let payload =
                        serde_json::to_string(&value).unwrap_or_else(|_| "<unserializable>".into());
                    eprintln!(
                        "codex-lsp: discarding unexpected payload while awaiting initialize response: {}",
                        payload
                    );
                }
            };
            if let Some(c) = init_value
                .get("result")
                .and_then(|res| res.get("capabilities"))
                .cloned()
            {
                self.server_capabilities = Some(c);
            }

            // Send initialized notification
            let initialized = json!({"jsonrpc":"2.0", "method":"initialized", "params": {}});
            self.write_jsonrpc(&initialized)?;
            Ok(())
        })();

        if let Err(e) = init_result {
            let _ = self.stop_child();
            return Err(e);
        }

        Ok(())
    }

    fn ensure_started(&mut self, override_cmd: Option<&str>) -> Result<()> {
        let override_cmd_owned = override_cmd.map(|s| s.to_string());

        let mut restart_needed = false;
        if let Some(child) = self.child.as_mut() {
            let child_has_exited = match child.try_wait() {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(e) => return Err(e.into()),
            };
            if child_has_exited {
                restart_needed = true;
            } else if let Some(ref override_cmd_str) = override_cmd_owned {
                if self.current_cmd.as_deref() != Some(override_cmd_str.as_str()) {
                    restart_needed = true;
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        }

        if restart_needed {
            self.stop_child()?;
        }

        let cmd = override_cmd_owned
            .clone()
            .or_else(|| self.current_cmd.clone())
            .or_else(|| self.default_cmd.clone())
            .ok_or_else(|| {
                anyhow!(
                    "No language server registered for this request. Provide arguments.serverCommand or configure LSP_SERVER_MAP overrides."
                )
            })?;

        if let Err(err) = self.start_server(&cmd) {
            eprintln!(
                "codex-lsp: failed to launch language server '{}': {err:#}",
                cmd
            );
            return Err(anyhow!(
                "failed to launch language server '{}': {:#}",
                cmd,
                err
            ));
        }

        self.current_cmd = Some(cmd);
        Ok(())
    }

    fn alloc_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        server_cmd: Option<&str>,
    ) -> Result<Value> {
        self.ensure_started(server_cmd)?;
        let id = self.alloc_id();
        let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.write_jsonrpc(&req)?;
        loop {
            let value = self.read_message().context("parse lsp response")?;

            if value.get("id") == Some(&json!(id)) {
                if let Some(err) = value.get("error") {
                    let formatted = self.format_lsp_error(method, err, server_cmd);
                    eprintln!("codex-lsp: {}", formatted);
                    return Err(formatted);
                }
                if let Some(result) = value.get("result") {
                    return Ok(result.clone());
                }
                return Err(anyhow!("LSP response missing result for id {id}"));
            }

            if let Some(method_name) = value.get("method").and_then(|m| m.as_str()) {
                if let Some(req_id) = value.get("id").cloned() {
                    if let Err(err) =
                        self.handle_server_request(req_id, method_name, value.get("params"))
                    {
                        eprintln!(
                            "codex-lsp: failed to handle server request '{}' while awaiting '{}': {err:#}",
                            method_name, method
                        );
                    }
                    continue;
                }
                eprintln!(
                    "codex-lsp: dropping unsolicited notification '{}' while awaiting '{}'",
                    method_name, method
                );
                continue;
            }

            if let Some(resp_id) = value.get("id") {
                eprintln!(
                    "codex-lsp: ignoring response for unexpected id {} while waiting for {}",
                    resp_id, id
                );
                continue;
            }

            if let Some(method_name) = value.get("method").and_then(|m| m.as_str()) {
                eprintln!(
                    "codex-lsp: dropping unsolicited notification '{}' while awaiting '{}'",
                    method_name, method
                );
            } else {
                let payload =
                    serde_json::to_string(&value).unwrap_or_else(|_| "<unserializable>".into());
                eprintln!(
                    "codex-lsp: dropping unexpected payload while awaiting '{}': {}",
                    method, payload
                );
            }
        }
    }

    pub fn notify(&mut self, method: &str, params: Value, server_cmd: Option<&str>) -> Result<()> {
        self.ensure_started(server_cmd)?;
        let notif = json!({"jsonrpc":"2.0","method": method, "params": params});
        self.write_jsonrpc(&notif)
    }

    pub fn capabilities(&mut self, server_cmd: Option<&str>) -> Result<Option<Value>> {
        match self.ensure_started(server_cmd) {
            Ok(()) => Ok(self.server_capabilities.clone()),
            Err(e) => {
                // If no server is configured, treat as no capabilities available.
                let msg = format!("{}", e);
                if msg.contains("No language server registered") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    pub fn shutdown(&mut self) -> Result<()> {
        self.stop_child()
    }
}

impl LanguageServerManager {
    fn format_lsp_error(
        &self,
        method: &str,
        err: &Value,
        server_cmd: Option<&str>,
    ) -> anyhow::Error {
        let server_label = server_cmd
            .map(|s| s.to_string())
            .or_else(|| self.current_cmd.clone())
            .or_else(|| self.default_cmd.clone());

        let mut msg = String::new();
        write!(&mut msg, "LSP request {}", method).ok();
        if let Some(server) = server_label.as_deref() {
            write!(&mut msg, " via '{}'", server).ok();
        }
        msg.push_str(" failed");

        let (code, message, data) = if let Some(obj) = err.as_object() {
            (
                obj.get("code").and_then(|c| c.as_i64()),
                obj.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string),
                obj.get("data").cloned(),
            )
        } else {
            (None, None, None)
        };

        if let Some(code) = code {
            write!(&mut msg, " (code {code})").ok();
        }
        if let Some(text) = message {
            if !text.is_empty() {
                write!(&mut msg, ": {text}").ok();
            }
        } else if let Some(text) = err.as_str() {
            if !text.is_empty() {
                write!(&mut msg, ": {text}").ok();
            }
        }

        let mut appended_detail = false;
        if let Some(detail) = data.filter(|d| !d.is_null()) {
            if let Ok(rendered) = serde_json::to_string(&detail) {
                if !rendered.is_empty() && rendered != "null" {
                    write!(&mut msg, "; details: {rendered}").ok();
                    appended_detail = true;
                }
            }
        } else if !err.is_object() {
            if let Ok(rendered) = serde_json::to_string(err) {
                if !rendered.is_empty() && rendered != "null" {
                    write!(&mut msg, "; details: {rendered}").ok();
                    appended_detail = true;
                }
            }
        }

        if !appended_detail {
            if let Ok(payload) = serde_json::to_string(err) {
                if !payload.is_empty() && payload != "null" {
                    write!(&mut msg, "; payload: {payload}").ok();
                }
            }
        }

        anyhow!(msg)
    }
}
