use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

fn write_content_length(mut writer: impl Write, value: &Value) {
    let body = serde_json::to_vec(value).expect("serialize request");
    write!(writer, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
    writer.write_all(&body).expect("write body");
    writer.flush().expect("flush");
}

fn read_content_length(reader: impl Read) -> Value {
    let mut reader = BufReader::new(reader);
    let mut header = String::new();
    let mut content_length: Option<usize> = None;
    loop {
        header.clear();
        let bytes = reader.read_line(&mut header).expect("read header line");
        if bytes == 0 {
            panic!("unexpected EOF reading headers");
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(rest) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = Some(rest.trim().parse().expect("parse content-length"));
        }
    }
    let len = content_length.expect("missing Content-Length header");
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).expect("read frame body");
    serde_json::from_slice(&buf).expect("decode json")
}

fn initialize_request(id: u64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": serde_json::json!({}),
            "clientInfo": {
                "name": "codex-orchestrator-tests",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    })
}

fn initialized_notification() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    })
}

#[test]
fn initialize_roundtrip_completes_within_timeout() {
    let bin = env!("CARGO_BIN_EXE_codex-orchestrator");
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn orchestrator");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let init_req = initialize_request(1);
    let initialized = initialized_notification();
    write_content_length(&mut stdin, &init_req);
    write_content_length(&mut stdin, &initialized);

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let value = read_content_length(stdout);
        let _ = tx.send(value);
    });

    let value = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("initialize response within timeout");
    let result = value
        .get("result")
        .cloned()
        .expect("response result present");

    assert_eq!(value.get("jsonrpc"), Some(&Value::String("2.0".into())));
    assert_eq!(result["serverInfo"]["name"], "codex-orchestrator");

    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
}
