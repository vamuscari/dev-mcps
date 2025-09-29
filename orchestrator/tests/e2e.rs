use std::io::{BufRead, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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

fn with_timeout<F: FnOnce() + Send + 'static>(dur: Duration, f: F) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let res = panic::catch_unwind(AssertUnwindSafe(f));
        let _ = tx.send(res.is_ok());
    });
    match rx.recv_timeout(dur) {
        Ok(true) => (),
        Ok(false) => panic!("test panicked"),
        Err(_) => panic!("test timed out after {:?}", dur),
    }
}

fn write_content_length(mut w: impl Write, body: &str) {
    let hdr = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(hdr.as_bytes()).unwrap();
    w.write_all(body.as_bytes()).unwrap();
}

fn read_content_length(r: impl Read) -> String {
    let mut reader = std::io::BufReader::new(r);
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).unwrap();
        assert!(n > 0, "unexpected EOF reading headers");
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(rest) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = Some(rest.trim().parse().unwrap());
        }
    }
    let len = content_length.expect("missing Content-Length");
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

#[test]
fn orchestrator_init_roundtrip_content_length() {
    with_timeout(Duration::from_secs(60), || {
        let bin = env!("CARGO_BIN_EXE_codex-orchestrator");
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn orchestrator");

        let mut stdin = child.stdin.take().unwrap();
        let init_req = initialize_request(1);
        let initialized = initialized_notification();
        write_content_length(&mut stdin, &serde_json::to_string(&init_req).unwrap());
        write_content_length(&mut stdin, &serde_json::to_string(&initialized).unwrap());
        stdin.flush().unwrap();

        let resp = read_content_length(child.stdout.take().unwrap());
        assert!(resp.contains("\"result\""));
        assert!(resp.contains("capabilities"));
        drop(stdin);
        let _ = child.wait();
    });
}
