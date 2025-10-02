use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Minimal DAP (Debug Adapter Protocol) client manager that speaks Content-Length framed JSON.
/// The DAP wire messages are not JSON-RPC 2.0; they use { type, seq, command, arguments } for
/// requests and { type: "response", request_seq, success, body } for responses. Events are
/// { type: "event", event, body } and can arrive at any time.
pub struct DapAdapterManager {
    cmd: Option<String>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<std::io::BufReader<ChildStdout>>,
    next_seq: i64,
    capabilities: Option<Value>,
}

impl DapAdapterManager {
    pub fn new() -> Self {
        let cmd = std::env::var("DAP_ADAPTER_CMD").ok();
        Self {
            cmd,
            child: None,
            stdin: None,
            stdout: None,
            next_seq: 1,
            capabilities: None,
        }
    }

    fn write_content_length(w: &mut ChildStdin, body: &str) -> Result<()> {
        write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
        w.write_all(body.as_bytes())?;
        w.flush()?;
        Ok(())
    }

    fn read_content_length(r: &mut std::io::BufReader<ChildStdout>) -> Result<String> {
        let mut content_length: Option<usize> = None;
        let mut line = String::new();
        loop {
            line.clear();
            let n = r.read_line(&mut line)?;
            if n == 0 {
                return Err(anyhow!("EOF from debug adapter"));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = Some(rest.trim().parse().context("parse content-length")?);
            }
        }
        let len = content_length.ok_or_else(|| anyhow!("missing Content-Length"))?;
        let mut buf = vec![0u8; len];
        use std::io::Read;
        r.read_exact(&mut buf)?;
        String::from_utf8(buf).context("utf8 body")
    }

    fn ensure_started(&mut self, override_cmd: Option<&str>) -> Result<()> {
        if self.child.is_some() {
            return Ok(());
        }
        let Some(cmd) = override_cmd
            .map(|s| s.to_string())
            .or_else(|| self.cmd.clone())
        else {
            return Err(anyhow!(
                "DAP adapter not configured. Set DAP_ADAPTER_CMD or pass arguments.adapterCommand."
            ));
        };
        let mut child = Command::new(cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawn dap adapter")?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        self.stdin = Some(stdin);
        self.stdout = Some(std::io::BufReader::new(stdout));
        self.child = Some(child);

        // Send initialize request
        let seq = self.alloc_seq();
            let init = json!({
                "seq": seq,
                "type": "request",
                "command": "initialize",
                "arguments": {
                "clientID": "mcp-dap",
                "adapterID": "mcp-dap",
                "pathFormat": "path",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "supportsRunInTerminalRequest": false
            }
        });
        let s = serde_json::to_string(&init)?;
        let w = self.stdin.as_mut().unwrap();
        Self::write_content_length(w, &s)?;

        // Read messages until the initialize response arrives.
        let r = self.stdout.as_mut().unwrap();
        loop {
            let body = Self::read_content_length(r)?;
            let v: Value = serde_json::from_str(&body).context("parse dap message")?;
            match (v.get("type").and_then(|x| x.as_str()), v.get("seq")) {
                (Some("response"), _) => {
                    let req_seq = v.get("request_seq").and_then(|x| x.as_i64());
                    let command = v.get("command").and_then(|x| x.as_str());
                    if req_seq == Some(seq) && command == Some("initialize") {
                        // Save capabilities as the body
                        self.capabilities = v.get("body").cloned();
                        break;
                    }
                }
                _ => {
                    // Ignore events and other traffic for now.
                }
            }
        }
        Ok(())
    }

    fn alloc_seq(&mut self) -> i64 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    pub fn request(
        &mut self,
        command: &str,
        arguments: Value,
        adapter_cmd: Option<&str>,
    ) -> Result<Value> {
        self.ensure_started(adapter_cmd)?;
        let seq = self.alloc_seq();
        let req = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments
        });
        let s = serde_json::to_string(&req)?;
        let w = self.stdin.as_mut().unwrap();
        let r = self.stdout.as_mut().unwrap();
        Self::write_content_length(w, &s)?;
        // Read until matching response; ignore events.
        loop {
            let body = Self::read_content_length(r)?;
            let v: Value = serde_json::from_str(&body).context("parse dap message")?;
            if v.get("type").and_then(|x| x.as_str()) == Some("response")
                && v.get("request_seq").and_then(|x| x.as_i64()) == Some(seq)
            {
                let ok = v.get("success").and_then(|x| x.as_bool()).unwrap_or(true);
                if ok {
                    return Ok(v.get("body").cloned().unwrap_or_else(|| json!({})));
                } else {
                    let msg = v
                        .get("message")
                        .and_then(|x| x.as_str())
                        .unwrap_or("dap error");
                    return Err(anyhow!("{}", msg));
                }
            }
        }
    }

    pub fn capabilities(&mut self, adapter_cmd: Option<&str>) -> Result<Option<Value>> {
        match self.ensure_started(adapter_cmd) {
            Ok(()) => Ok(self.capabilities.clone()),
            Err(e) => {
                let msg = format!("{}", e);
                if msg.contains("DAP adapter not configured") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }
}
