//! 假的 Purrdr 後台(本機 HTTP):記下 `POST /v1/push` 的本文，依密鑰回指定的狀態碼與本文(預設 200)。
//! 錯誤本文的形狀同 raychiutw/purrdr.server:`{"error": {"code", "message"}}`。

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

type Received = Arc<Mutex<Vec<(String, Value)>>>;
type StatusByKey = Arc<Mutex<BTreeMap<String, (u16, Option<String>)>>>;

/// 測試結束(drop)時停止的假後台。
pub struct FakePushBackend {
    address: String,
    received: Received,
    status_by_key: StatusByKey,
    stopped: Arc<AtomicBool>,
}

impl FakePushBackend {
    pub fn start() -> FakePushBackend {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let received: Received = Arc::default();
        let status_by_key: StatusByKey = Arc::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let (thread_received, thread_status, thread_stopped) = (received.clone(), status_by_key.clone(), stopped.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_stopped.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    respond(stream, &thread_received, &thread_status);
                }
            }
        });
        FakePushBackend {
            address,
            received,
            status_by_key,
            stopped,
        }
    }

    /// 配對檔的 `endpoint=`。
    pub fn endpoint(&self) -> String {
        format!("{}/v1/push", self.address)
    }

    /// 帶這把密鑰的請求回 `status`(本文是預設的)。
    pub fn respond_to_key(&self, key: &str, status: u16) {
        self.status_by_key.lock().unwrap().insert(key.to_owned(), (status, None));
    }

    /// 帶這把密鑰的請求回 `status` 與 `body`。
    pub fn respond_to_key_with(&self, key: &str, status: u16, body: &str) {
        self.status_by_key.lock().unwrap().insert(key.to_owned(), (status, Some(body.to_owned())));
    }

    /// 收到的請求(路徑、JSON 本文)。
    pub fn received(&self) -> Vec<(String, Value)> {
        self.received.lock().unwrap().clone()
    }

    /// 收到的本文。
    pub fn bodies(&self) -> Vec<Value> {
        self.received().into_iter().map(|(_, body)| body).collect()
    }
}

impl Drop for FakePushBackend {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address.trim_start_matches("http://"));
    }
}

fn respond(mut stream: TcpStream, received: &Received, status_by_key: &StatusByKey) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut content_length = 0;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; content_length];
    let _ = reader.read_exact(&mut body);
    let path = request_line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    // 主機心跳(/v1/heartbeat,#99)與推播無關:不記入、不套用依密鑰設定的狀態，一律回「還需要」。
    if path == "/v1/heartbeat" {
        let response_body = r#"{"needed":true}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(response_body.as_bytes());
        return;
    }
    let (status, configured_body) = json["key"]
        .as_str()
        .and_then(|key| status_by_key.lock().unwrap().get(key).cloned())
        .unwrap_or((200, None));
    received.lock().unwrap().push((path, json));
    let response_body = configured_body.unwrap_or_else(|| {
        if status == 200 {
            r#"{"delivered":true}"#.to_owned()
        } else {
            r#"{"error":{"code":"pairing_revoked","message":"This pairing was revoked."}}"#.to_owned()
        }
    });
    // 3xx 時轉址回同一個路徑(測試主機服務不跟隨轉址)。
    let location = if (300..400).contains(&status) { "Location: /v1/push\r\n" } else { "" };
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(response_body.as_bytes());
}
