//! 超大對話檔(raychiutw/purrdr.host#108):63 MB 的 Claude 對話檔讓 HTTPS 聊天列表沒有時間、context 環與預覽。
//! 這裡以 64 MB 以上的合成對話檔(結尾有可讀的 user／assistant 訊息)跑 locate、previews、window，
//! 斷言都在期限內回傳、預覽是最後一則可讀訊息、只讀檔尾需要的範圍(以讀取位元組數計)、超過上限回明確錯誤碼，
//! 以及檔案持續寫入中邊寫邊讀的結果正確。

// 測試以全域鎖序列化(讀取位元組計數器是全域的),鎖會跨 await 持有。
#![allow(clippy::await_holding_lock)]

mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use purrdr_host_service::transcript::{self, BYTES_READ, TranscriptEnvironment};
use purrdr_host_service::transcript_reads;
use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, TestHostService, recorded, start_service_with_transcripts};
use tempfile::TempDir;

const CLAUDE_SESSION: &str = "00000000-0000-0000-0000-000000000001";
const CLAUDE_PROJECT_FOLDER: &str = "-Users-demo-Projects-garden-journal";

const MEGABYTE: u64 = 1 << 20;
/// 合成檔的最小大小(issue 的檔案是 63 MB、26,081 行)。
const LARGE_FILE_BYTES: u64 = 66 * MEGABYTE;
/// 每個請求的期限。
const DEADLINE: Duration = Duration::from_secs(10);
/// 檔尾兩行工具輸出各自的大小。
const HUGE_LINE_BYTES: usize = 3 * MEGABYTE as usize;

/// 讀取位元組數的上限:預覽 8 MB 再加餘裕、視窗 16 MB 再加一個區塊的餘裕。
const PREVIEW_READ_LIMIT: u64 = 12 * MEGABYTE;
const WINDOW_READ_LIMIT: u64 = 24 * MEGABYTE;

/// `BYTES_READ` 是整個行程共用的計數:要斷言讀取量的測試一次只跑一個。
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn bytes_read() -> u64 {
    BYTES_READ.load(Ordering::Relaxed)
}

/// 一行 Claude 對話紀錄(約 2.2 KB,有時間與序號;文字有空白,不會被當成 base64 剝除)。
fn message_line(sequence: u64, role: &str, text: &str) -> String {
    let padding = "lorem ipsum dolor sit amet ".repeat(80);
    format!(
        "{{\"type\":\"{role}\",\"sequence\":{sequence},\"timestamp\":\"2026-11-28T10:{:02}:{:02}.000Z\",\"message\":{{\"role\":\"{role}\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}},\"pad\":\"{padding}\"}}\n",
        (sequence / 60) % 60,
        sequence % 60
    )
}

/// 工具輸出那種超大的一行(`size` bytes 左右,文字有空白)。
fn huge_line(sequence: u64, size: usize) -> String {
    let body = "tool output line with spaces ".repeat(size / 29);
    format!(
        "{{\"type\":\"user\",\"sequence\":{sequence},\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"content\":\"{body}\"}}]}}}}\n"
    )
}

/// 寫一份至少 `LARGE_FILE_BYTES` 的對話檔:一般訊息，最後是可讀的 assistant 訊息，之後跟著兩行各約 3 MB 的工具輸出。
/// 回傳最後一則可讀訊息的序號。
fn write_large_transcript(path: &Path) -> u64 {
    write_transcript(path, true)
}

/// `huge_tail` 為假時檔案就結束在可讀訊息。
fn write_transcript(path: &Path, huge_tail: bool) -> u64 {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut written = 0_u64;
    let mut sequence = 0_u64;
    while written < LARGE_FILE_BYTES {
        let role = if sequence.is_multiple_of(2) { "user" } else { "assistant" };
        let line = message_line(sequence, role, &format!("message {sequence}"));
        file.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
        sequence += 1;
    }
    let readable = message_line(sequence, "assistant", "LAST_READABLE_MESSAGE");
    file.write_all(readable.as_bytes()).unwrap();
    for offset in 1..=if huge_tail { 2 } else { 0 } {
        file.write_all(huge_line(sequence + offset, HUGE_LINE_BYTES).as_bytes()).unwrap();
    }
    file.flush().unwrap();
    sequence
}

struct Fixture {
    home: TempDir,
    _herdr: FakeHerdr,
    service: TestHostService,
}

impl Fixture {
    async fn start() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let herdr = FakeHerdr::start(vec![
            ("ping", recorded("ping.ndjson")),
            ("session.snapshot", recorded("session.snapshot.ndjson")),
        ])
        .await;
        let service = start_service_with_transcripts(
            herdr.socket.clone(),
            TranscriptEnvironment {
                home: Some(home.path().to_owned()),
                ..TranscriptEnvironment::default()
            },
        )
        .await;
        Fixture { home, _herdr: herdr, service }
    }

    fn transcript_path(&self) -> PathBuf {
        let folder = self.home.path().join(".claude").join("projects").join(CLAUDE_PROJECT_FOLDER);
        std::fs::create_dir_all(&folder).unwrap();
        folder.join(format!("{CLAUDE_SESSION}.jsonl"))
    }

    async fn client(&self) -> TestClient {
        let mut client = TestClient::connect(&self.service).await;
        let welcome = client.request(json!({"type": "hello", "request_id": "r0", "protocol_version": 1})).await;
        assert_eq!(welcome["type"], "welcome");
        client
    }
}

/// 送出請求並限時等回覆;回覆與花的時間。
async fn timed(client: &mut TestClient, request: Value) -> (Value, Duration) {
    let started = Instant::now();
    let reply = tokio::time::timeout(DEADLINE, client.request(request))
        .await
        .unwrap_or_else(|_| panic!("沒有在 {DEADLINE:?} 內回覆"));
    (reply, started.elapsed())
}

fn locate_request() -> Value {
    json!({"type": "transcript.locate", "request_id": "locate", "session": {"kind": "id", "value": CLAUDE_SESSION}, "header": false})
}

fn previews_request(path: &Path) -> Value {
    json!({"type": "transcript.previews", "request_id": "previews", "entries": [{"key": "w3:p1", "path": path.to_string_lossy()}]})
}

fn window_request(path: &Path, end: u64, lines: u64) -> Value {
    json!({"type": "transcript.window", "request_id": "window", "path": path.to_string_lossy(), "end": end, "lines": lines})
}

/// 回覆太長時只印開頭,避免失敗訊息塞滿終端機。
fn brief(reply: &Value) -> String {
    reply.to_string().chars().take(200).collect()
}

#[tokio::test]
async fn locate_on_a_64_mb_transcript_returns_in_time_without_reading_the_file() {
    let _serial = serial();
    let fixture = Fixture::start().await;
    let path = fixture.transcript_path();
    write_large_transcript(&path);
    let mut client = fixture.client().await;
    let before = bytes_read();

    let (reply, elapsed) = timed(&mut client, locate_request()).await;

    assert_eq!(reply["type"], "transcript.locate.result", "{}", brief(&reply));
    assert!(reply["size"].as_u64().unwrap() >= LARGE_FILE_BYTES);
    assert!(elapsed < DEADLINE, "locate 花了 {elapsed:?}");
    assert!(bytes_read() - before <= MEGABYTE, "locate 讀了 {} bytes", bytes_read() - before);
}

#[tokio::test]
async fn previews_of_a_64_mb_transcript_end_with_the_last_readable_message_and_read_a_bounded_tail() {
    let _serial = serial();
    let fixture = Fixture::start().await;
    let path = fixture.transcript_path();
    let readable = write_large_transcript(&path);
    let mut client = fixture.client().await;
    client.request(locate_request()).await;
    let before = bytes_read();

    let (reply, elapsed) = timed(&mut client, previews_request(&path)).await;

    let read = bytes_read() - before;
    assert!(elapsed < DEADLINE, "previews 花了 {elapsed:?}");
    assert!(read <= PREVIEW_READ_LIMIT, "previews 讀了 {read} bytes");
    let records = reply["previews"][0]["records"]
        .as_array()
        .unwrap_or_else(|| panic!("沒有預覽:{}", brief(&reply)));
    let last_readable = records
        .iter()
        .filter_map(|record| serde_json::from_str::<Value>(record["text"].as_str()?).ok())
        .rfind(|line| line["message"]["content"][0]["type"] == "text")
        .unwrap_or_else(|| panic!("預覽裡沒有任何可讀的訊息:{} 筆記錄", records.len()));
    assert_eq!(last_readable["sequence"], readable);
    assert_eq!(last_readable["message"]["content"][0]["text"], "LAST_READABLE_MESSAGE");
    let response_bytes = reply.to_string().len() as u64;
    assert!(response_bytes <= MEGABYTE, "預覽回應 {response_bytes} bytes");
}

#[tokio::test]
async fn window_of_a_64_mb_transcript_returns_the_last_lines_in_time_with_a_bounded_read() {
    let _serial = serial();
    let fixture = Fixture::start().await;
    let path = fixture.transcript_path();
    let readable = write_large_transcript(&path);
    let size = std::fs::metadata(&path).unwrap().len();
    let mut client = fixture.client().await;
    client.request(locate_request()).await;
    let before = bytes_read();

    let (reply, elapsed) = timed(&mut client, window_request(&path, size, 40)).await;

    assert_eq!(reply["type"], "transcript.window.result", "{}", brief(&reply));
    assert!(elapsed < DEADLINE, "window 花了 {elapsed:?}");
    assert!(bytes_read() - before <= WINDOW_READ_LIMIT, "window 讀了 {} bytes", bytes_read() - before);
    let records = reply["records"].as_array().unwrap();
    assert_eq!(records.len(), 40);
    assert_eq!(reply["end"], size);
    let text = records[records.len() - 3]["text"].as_str().unwrap();
    assert!(text.contains("LAST_READABLE_MESSAGE") && text.contains(&format!("\"sequence\":{readable}")));
}

#[tokio::test]
async fn window_asking_for_more_lines_than_the_read_limit_allows_reports_transcript_too_large() {
    let _serial = serial();
    let fixture = Fixture::start().await;
    let path = fixture.transcript_path();
    write_large_transcript(&path);
    let size = std::fs::metadata(&path).unwrap().len();
    let mut client = fixture.client().await;
    client.request(locate_request()).await;
    let before = bytes_read();

    let (reply, elapsed) = timed(&mut client, window_request(&path, size, 1_000_000)).await;

    assert!(elapsed < DEADLINE, "window 花了 {elapsed:?}");
    assert!(bytes_read() - before <= WINDOW_READ_LIMIT, "window 讀了 {} bytes", bytes_read() - before);
    assert_eq!(reply["type"], "error", "{}", brief(&reply));
    assert_eq!(reply["code"], "transcript_too_large");
    assert_eq!(reply["request_id"], "window");
}

#[tokio::test]
async fn window_with_max_bytes_still_returns_what_fits_on_a_64_mb_transcript() {
    let _serial = serial();
    let fixture = Fixture::start().await;
    let path = fixture.transcript_path();
    write_large_transcript(&path);
    let size = std::fs::metadata(&path).unwrap().len();
    let mut client = fixture.client().await;
    client.request(locate_request()).await;
    let before = bytes_read();
    let mut request = window_request(&path, size, 1_000_000);
    request["max_bytes"] = json!(MEGABYTE);

    let (reply, _) = timed(&mut client, request).await;

    assert_eq!(reply["type"], "transcript.window.result", "{}", brief(&reply));
    assert!(bytes_read() - before <= 2 * MEGABYTE);
}

/// 對話檔持續寫入中:寫入端每行分兩次寫(模擬寫到一半),讀取端不斷跑視窗與預覽。
/// 視窗只會回完整的行、序號連續;預覽的最新一則不會比讀取前已寫完的更舊。
#[test]
fn reading_while_the_transcript_is_being_written_returns_only_complete_and_current_lines() {
    let _serial = serial();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("growing.jsonl");
    let readable = write_transcript(&path, false);
    let first_new = readable + 1;

    let published = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (path, published, done) = (path.clone(), Arc::clone(&published), Arc::clone(&done));
        std::thread::spawn(move || {
            let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            for sequence in first_new..first_new + 300 {
                let line = message_line(sequence, "assistant", &format!("message {sequence}"));
                let (first, second) = line.split_at(line.len() / 2);
                file.write_all(first.as_bytes()).unwrap();
                file.flush().unwrap();
                std::thread::sleep(Duration::from_micros(300));
                file.write_all(second.as_bytes()).unwrap();
                file.flush().unwrap();
                published.store(sequence, Ordering::SeqCst);
            }
            done.store(true, Ordering::SeqCst);
        })
    };

    let mut rounds = 0;
    loop {
        let finished = done.load(Ordering::SeqCst);
        rounds += 1;
        let known = published.load(Ordering::SeqCst);
        let size = std::fs::metadata(&path).unwrap().len();
        // 只取寫入端寫的行(前面截斷造成的殘缺行不在視窗內:視窗 20 行都在新寫入的行之後才驗,所以等寫夠 30 行)。
        if known >= first_new + 30 {
            let chunk = transcript::window(&path, size, 20, None).unwrap();
            let sequences: Vec<u64> = chunk
                .records
                .iter()
                .map(|record| {
                    let line: Value = serde_json::from_str(&record.text)
                        .unwrap_or_else(|_| panic!("視窗回了不完整的行:{}", record.text.chars().take(80).collect::<String>()));
                    line["sequence"].as_u64().unwrap()
                })
                .collect();
            assert_eq!(sequences.len(), 20);
            assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1), "序號不連續:{sequences:?}");
            assert!(
                sequences.last().unwrap() >= &known,
                "視窗的最後一行 {} 比讀取前已寫完的 {known} 舊",
                sequences.last().unwrap()
            );
            let preview = transcript_reads::preview(&path, None).unwrap();
            let newest = preview
                .records
                .iter()
                .filter_map(|record| serde_json::from_str::<Value>(&record.text).ok())
                .filter_map(|line| line["sequence"].as_u64())
                .max()
                .expect("預覽沒有任何完整的訊息");
            assert!(newest >= known, "預覽的最新一則 {newest} 比讀取前已寫完的 {known} 舊");
        }
        if finished {
            break;
        }
    }
    writer.join().unwrap();
    assert!(rounds > 1);
}
