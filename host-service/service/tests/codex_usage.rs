//! Codex 的方案用量(raychiutw/purrdr.host#62):`usage.codex` 由主機服務執行一次 `codex app-server`,
//! 讀 `account/rateLimits/read`(與 app 經 SSH 的 `CodexPlanUsageHost` 同一套交談)。
//!
//! 假的 codex 就是這支測試執行檔本身(`fake-codex <資料夾> app-server`):依資料夾裡 `scenario` 的內容回應，
//! 收到的每一行記在 `received`。所以不用 libtest(`harness = false`)。

mod support;

use std::future::Future;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::pin::Pin;

use purrdr_host_service::codex_usage::CodexCli;
use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, TestHostService, recorded, start_service_with_codex_cli};
use tempfile::TempDir;

const FAKE_CODEX: &str = "fake-codex";

// MARK: - 假的 codex

/// `fake-codex <資料夾> app-server`:一行一則 JSON-RPC(不帶 jsonrpc 欄位,同 codex app-server 的 stdio)。
fn fake_codex(arguments: &[String]) -> i32 {
    let directory = PathBuf::from(&arguments[0]);
    assert_eq!(arguments[1..], ["app-server".to_owned()], "只執行 app-server");
    let scenario = std::fs::read_to_string(directory.join("scenario")).unwrap_or_default();
    if scenario.trim() == "exit" {
        return 3;
    }
    std::fs::write(directory.join("path"), std::env::var("PATH").unwrap_or_default()).unwrap();
    let mut received = std::fs::OpenOptions::new().create(true).append(true).open(directory.join("received")).unwrap();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        writeln!(received, "{line}").unwrap();
        let message: Value = serde_json::from_str(&line).unwrap();
        let reply = match (message["method"].as_str(), message["id"].as_i64()) {
            (Some("initialize"), Some(id)) => json!({"id": id, "result": {"userAgent": "codex/0.1"}}),
            (Some("account/rateLimits/read"), Some(id)) => {
                // 回應前先送一則通知，與一則 id 相同的 app-server 請求：主機服務只收回應。
                writeln!(out, "{}", json!({"method": "account/updated", "params": {}})).unwrap();
                writeln!(out, "{}", json!({"method": "item/tool/requestUserInput", "id": id, "params": {}})).unwrap();
                rate_limits_reply(scenario.trim(), id)
            }
            _ => continue,
        };
        writeln!(out, "{reply}").unwrap();
        out.flush().unwrap();
    }
    0
}

fn rate_limits_reply(scenario: &str, id: i64) -> Value {
    match scenario {
        "buckets" => json!({"id": id, "result": {
            "rateLimits": {"primary": {"usedPercent": 99.0}},
            "rateLimitsByLimitId": {
                "other": {"primary": {"usedPercent": 88.0}},
                "codex": {
                    "primary": {"usedPercent": 42.4, "windowDurationMins": 300, "resetsAt": 1_795_000_000},
                    "secondary": {"usedPercent": 12.6, "windowDurationMins": 10080}
                }
            }
        }}),
        "single" => json!({"id": id, "result": {"rateLimits": {
            "primary": {"usedPercent": 150.0},
            "secondary": {"usedPercent": -3.0, "windowDurationMins": 10080, "resetsAt": 1_795_000_000}
        }}}),
        "remap" => json!({"id": id, "result": {"rateLimits": {
            "primary": {"usedPercent": 70.0, "windowDurationMins": 10080, "resetsAt": 1_795_000_000.6},
            "secondary": {"usedPercent": 5.0, "windowDurationMins": 10080}
        }}}),
        "not-signed-in" => json!({"id": id, "error": {"code": -32600, "message": "authentication required to read rate limits"}}),
        "fails" => json!({"id": id, "error": {"code": -32603, "message": "upstream timeout"}}),
        "unreadable" => json!({"id": id, "result": {"something": "else"}}),
        other => panic!("unknown scenario {other}"),
    }
}

// MARK: - 測試環境

struct Harness {
    _herdr: FakeHerdr,
    fake: TempDir,
    service: TestHostService,
}

impl Harness {
    async fn start(scenario: &str) -> Harness {
        let fake = tempfile::tempdir().unwrap();
        std::fs::write(fake.path().join("scenario"), scenario).unwrap();
        let codex = CodexCli::at_path(
            std::env::current_exe().unwrap(),
            vec![FAKE_CODEX.to_owned(), fake.path().to_string_lossy().into_owned()],
        );
        Harness::with_codex(codex, fake).await
    }

    async fn with_codex(codex: CodexCli, fake: TempDir) -> Harness {
        let herdr = FakeHerdr::start(vec![("ping", recorded("ping.ndjson"))]).await;
        let service = start_service_with_codex_cli(herdr.socket.clone(), codex).await;
        Harness { _herdr: herdr, fake, service }
    }

    async fn request(&self) -> Value {
        let mut client = TestClient::connect(&self.service).await;
        client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;
        client.request(json!({"type": "usage.codex", "request_id": "u1"})).await
    }

    /// 假 codex 收到的 JSON-RPC 方法(依序)。
    fn received_methods(&self) -> Vec<String> {
        std::fs::read_to_string(self.fake.path().join("received"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap()["method"].as_str().unwrap().to_owned())
            .collect()
    }
}

fn is_rfc3339_seconds(text: &str) -> bool {
    text.len() == 20 && text.ends_with('Z') && text.as_bytes()[10] == b'T'
}

// MARK: - 測試

async fn reads_the_codex_bucket() {
    let harness = Harness::start("buckets").await;

    let reply = harness.request().await;

    assert_eq!(reply["type"], "usage.codex.result", "{reply}");
    assert_eq!(reply["request_id"], "u1");
    assert!(is_rfc3339_seconds(reply["captured_at"].as_str().unwrap()), "{reply}");
    assert_eq!(
        reply["periods"],
        json!([
            {"kind": "five_hour", "used_percentage": 42.4, "resets_at": "2026-11-18T11:06:40Z"},
            {"kind": "week", "used_percentage": 12.6}
        ])
    );
    assert_eq!(harness.received_methods(), ["initialize", "initialized", "account/rateLimits/read"]);
}

async fn without_buckets_reads_rate_limits_and_clamps_percentages() {
    let harness = Harness::start("single").await;

    let reply = harness.request().await;

    assert_eq!(
        reply["periods"],
        json!([
            {"kind": "five_hour", "used_percentage": 100.0},
            {"kind": "week", "used_percentage": 0.0, "resets_at": "2026-11-18T11:06:40Z"}
        ]),
        "{reply}"
    );
}

async fn window_length_decides_the_kind_and_duplicates_are_skipped() {
    let harness = Harness::start("remap").await;

    let reply = harness.request().await;

    // primary 是一週長：算 week;secondary 也是 week,重複的略過。重置時間帶小數也收。
    assert_eq!(
        reply["periods"],
        json!([{"kind": "week", "used_percentage": 70.0, "resets_at": "2026-11-18T11:06:40Z"}]),
        "{reply}"
    );
}

/// 背景服務的 PATH 只有系統目錄;npm 裝的 codex 是 `#!/usr/bin/env node`,要在 codex 所在的資料夾找得到 node。
async fn codex_runs_with_its_own_folder_first_on_path() {
    let harness = Harness::start("buckets").await;

    harness.request().await;

    let path = std::fs::read_to_string(harness.fake.path().join("path")).unwrap();
    let first = std::env::split_paths(&path).next().unwrap();
    assert_eq!(first, std::env::current_exe().unwrap().parent().unwrap(), "{path}");
}

async fn failures_have_their_own_error_codes() {
    let not_signed_in = Harness::start("not-signed-in").await.request().await;
    let fails = Harness::start("fails").await.request().await;
    let unreadable = Harness::start("unreadable").await.request().await;
    let exits = Harness::start("exit").await.request().await;
    let missing = Harness::with_codex(CodexCli::at_path(PathBuf::from("/nonexistent/codex"), Vec::new()), tempfile::tempdir().unwrap())
        .await
        .request()
        .await;

    assert_eq!(not_signed_in["code"], "codex_not_signed_in", "{not_signed_in}");
    assert_eq!(fails["code"], "codex_failed", "{fails}");
    assert_eq!(fails["message"], "upstream timeout");
    assert_eq!(unreadable["code"], "codex_failed", "{unreadable}");
    assert_eq!(exits["code"], "codex_failed", "{exits}");
    assert_eq!(missing["code"], "codex_not_found", "{missing}");
}

async fn read_only_devices_can_read_usage() {
    let harness = Harness::start("buckets").await;
    let device = harness.service.pair_device("借出的 iPad", true);
    let mut client = TestClient::connect_with_token(&harness.service, Some(&device.device_token)).await;
    client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;

    let reply = client.request(json!({"type": "usage.codex", "request_id": "u1"})).await;

    assert_eq!(reply["type"], "usage.codex.result", "{reply}");
}

// MARK: - 執行

type TestCase = (&'static str, fn() -> Pin<Box<dyn Future<Output = ()>>>);

fn tests() -> Vec<TestCase> {
    vec![
        ("reads_the_codex_bucket", || Box::pin(reads_the_codex_bucket())),
        ("window_length_decides_the_kind_and_duplicates_are_skipped", || {
            Box::pin(window_length_decides_the_kind_and_duplicates_are_skipped())
        }),
        ("codex_runs_with_its_own_folder_first_on_path", || {
            Box::pin(codex_runs_with_its_own_folder_first_on_path())
        }),
        ("without_buckets_reads_rate_limits_and_clamps_percentages", || {
            Box::pin(without_buckets_reads_rate_limits_and_clamps_percentages())
        }),
        ("failures_have_their_own_error_codes", || Box::pin(failures_have_their_own_error_codes())),
        ("read_only_devices_can_read_usage", || Box::pin(read_only_devices_can_read_usage())),
    ]
}

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some(FAKE_CODEX) {
        return std::process::ExitCode::from(fake_codex(&arguments[1..]) as u8);
    }
    let filters: Vec<&String> = arguments.iter().filter(|argument| !argument.starts_with('-')).collect();
    let selected: Vec<TestCase> = tests()
        .into_iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|filter| name.contains(filter.as_str())))
        .collect();
    println!("\nrunning {} tests", selected.len());
    let mut failures = Vec::new();
    for (name, test) in &selected {
        let outcome = std::panic::catch_unwind(|| {
            tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(test());
        });
        println!("test {name} ... {}", if outcome.is_ok() { "ok" } else { "FAILED" });
        if outcome.is_err() {
            failures.push(*name);
        }
    }
    println!("\ntest result: {} passed; {} failed", selected.len() - failures.len(), failures.len());
    if failures.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        println!("failures: {failures:?}");
        std::process::ExitCode::FAILURE
    }
}
