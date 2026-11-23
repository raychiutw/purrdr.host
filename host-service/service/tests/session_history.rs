//! 續接歷史 session(raychiutw/purrdr.host#32):sessions.list 列出工作資料夾過去的 Claude／Codex session,
//! agent.start 的 resume_session 轉成各 Agent 的續接參數，且只續接存在的 session。

mod support;

use std::path::Path;
use std::time::{Duration, SystemTime};

use purrdr_host_service::transcript::{TranscriptEnvironment, claude_project_folder};
use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, TestHostService, recorded, start_service_with_transcripts};
use tempfile::TempDir;

fn hello(request_id: &str) -> Value {
    json!({"type": "hello", "request_id": request_id, "protocol_version": 1})
}

const OLDER: &str = "0b9e6f1c-2d4a-4c8e-9f00-5a1b2c3d4e5f";
const NEWER: &str = "48b84c34-58a1-465f-94b2-370ed0699957";

fn project_folder() -> String {
    if cfg!(windows) {
        r"C:\Users\demo\Projects\garden-journal".to_owned()
    } else {
        "/Users/demo/Projects/garden-journal".to_owned()
    }
}

fn write(path: &Path, lines: &[Value], modified: SystemTime) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(path, text).unwrap();
    std::fs::File::options().write(true).open(path).unwrap().set_modified(modified).unwrap();
}

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn user(content: Value) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": content}})
}

/// 暫存家目錄裡的 Claude 專案資料夾：兩個 session,加上應略過的資料夾與檔案。
fn claude_home() -> TempDir {
    let home = TempDir::new().unwrap();
    let projects = home.path().join(".claude").join("projects").join(claude_project_folder(&project_folder()));
    write(
        &projects.join(format!("{OLDER}.jsonl")),
        &[
            json!({"type": "permission-mode", "permissionMode": "default"}),
            json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "<local-command-caveat>ignored</local-command-caveat>"}}),
            user(json!("<command-name>/clear</command-name>")),
            user(json!([{"type": "text", "text": "Add a daily   watering\nreminder to README.md"}])),
            user(json!("a later message")),
        ],
        at(1_763_600_000),
    );
    write(
        &projects.join(format!("{NEWER}.jsonl")),
        &[
            user(json!([{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}])),
            user(json!("x".repeat(300))),
        ],
        at(1_763_690_000),
    );
    std::fs::create_dir_all(projects.join("memory")).unwrap();
    write(&projects.join("notes.txt"), &[json!("not a transcript")], at(1_763_700_000));
    write(&projects.join("not safe.jsonl"), &[user(json!("skipped"))], at(1_763_700_000));
    home
}

fn environment(home: &TempDir) -> TranscriptEnvironment {
    TranscriptEnvironment {
        home: Some(home.path().to_owned()),
        ..TranscriptEnvironment::default()
    }
}

async fn connected(herdr: &FakeHerdr, home: &TempDir) -> (TestHostService, TestClient) {
    let service = start_service_with_transcripts(herdr.socket.clone(), environment(home)).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;
    (service, client)
}

async fn plain_herdr() -> FakeHerdr {
    // herdr 0.9.5 在這個平台錄的 resume 啟動(Windows 錄自 Windows 主機)。
    let started = if cfg!(windows) {
        recorded("windows/agent.start.resume.ndjson")
    } else {
        recorded("agent.start.resume.ndjson")
    };
    FakeHerdr::start(vec![("ping", recorded("ping.ndjson")), ("agent.start", started)]).await
}

#[tokio::test]
async fn claude_sessions_of_the_folder_newest_first_with_the_first_user_message() {
    let home = claude_home();
    let herdr = plain_herdr().await;
    let (_service, mut client) = connected(&herdr, &home).await;

    let reply = client
        .request(json!({"type": "sessions.list", "request_id": "r2", "cwd": project_folder(), "agent_kind": "claude"}))
        .await;

    assert_eq!(
        reply,
        json!({
            "type": "sessions.list.result",
            "request_id": "r2",
            "sessions": [
                {"id": NEWER, "modified": "2025-11-21T01:53:20Z", "summary": "x".repeat(120)},
                {"id": OLDER, "modified": "2025-11-20T00:53:20Z", "summary": "Add a daily watering reminder to README.md"},
            ],
        })
    );
}

#[tokio::test]
async fn codex_sessions_are_the_rollouts_whose_cwd_is_the_folder() {
    let home = TempDir::new().unwrap();
    let day = home.path().join(".codex").join("sessions").join("2025").join("11").join("20");
    let meta = |id: &str, cwd: &str| json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd}});
    write(
        &day.join(format!("rollout-2025-11-20T08-00-00-{OLDER}.jsonl")),
        &[
            meta(OLDER, &project_folder()),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>ignored</environment_context>"}]}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "Fix the flaky watering test"}}),
        ],
        at(1_763_600_000),
    );
    write(
        &day.join(format!("rollout-2025-11-20T09-00-00-{NEWER}.jsonl")),
        &[meta(NEWER, "/Users/demo/Projects/elsewhere")],
        at(1_763_690_000),
    );
    let herdr = plain_herdr().await;
    let (_service, mut client) = connected(&herdr, &home).await;

    let reply = client
        .request(json!({"type": "sessions.list", "request_id": "r2", "cwd": project_folder(), "agent_kind": "codex"}))
        .await;

    assert_eq!(
        reply["sessions"],
        json!([{"id": OLDER, "modified": "2025-11-20T00:53:20Z", "summary": "Fix the flaky watering test"}])
    );
}

#[tokio::test]
async fn a_folder_without_sessions_lists_nothing_and_relative_folders_are_rejected() {
    let home = TempDir::new().unwrap();
    let herdr = plain_herdr().await;
    let (_service, mut client) = connected(&herdr, &home).await;

    let empty = client
        .request(json!({"type": "sessions.list", "request_id": "r2", "cwd": project_folder(), "agent_kind": "claude"}))
        .await;
    let relative = client
        .request(json!({"type": "sessions.list", "request_id": "r3", "cwd": "Projects/garden-journal", "agent_kind": "claude"}))
        .await;

    assert_eq!(empty, json!({"type": "sessions.list.result", "request_id": "r2", "sessions": []}));
    assert_eq!(relative["code"], "invalid_message");
}

fn forwarded_start(herdr: &FakeHerdr) -> Value {
    herdr
        .requests()
        .into_iter()
        .find(|request| request["method"] == "agent.start")
        .expect("沒有送出 agent.start")["params"]
        .clone()
}

#[tokio::test]
async fn resuming_claude_passes_resume_after_the_permission_mode() {
    let home = claude_home();
    let herdr = plain_herdr().await;
    let (_service, mut client) = connected(&herdr, &home).await;

    let reply = client
        .request(json!({
            "type": "agent.start", "request_id": "r2", "pane_id": "w9:p1", "name": "garden-resume", "kind": "claude",
            "permission_mode": "plan", "resume_session": OLDER,
        }))
        .await;

    assert_eq!(reply, json!({"type": "agent.start.result", "request_id": "r2"}));
    assert_eq!(
        forwarded_start(&herdr),
        json!({"name": "garden-resume", "kind": "claude", "pane_id": "w9:p1", "args": ["--permission-mode", "plan", "--resume", OLDER]})
    );
}

#[tokio::test]
async fn resuming_codex_puts_no_daemon_before_the_resume_subcommand() {
    let home = TempDir::new().unwrap();
    let rollout = home
        .path()
        .join(".codex")
        .join("sessions")
        .join("2025")
        .join("11")
        .join("20")
        .join(format!("rollout-2025-11-20T08-00-00-{OLDER}.jsonl"));
    write(
        &rollout,
        &[json!({"type": "session_meta", "payload": {"id": OLDER, "cwd": project_folder()}})],
        at(1_763_600_000),
    );
    let herdr = plain_herdr().await;
    let (_service, mut client) = connected(&herdr, &home).await;

    let reply = client
        .request(json!({"type": "agent.start", "request_id": "r2", "pane_id": "w9:p1", "name": "garden-codex", "kind": "codex", "resume_session": OLDER}))
        .await;

    assert_eq!(reply["type"], "agent.start.result", "{reply}");
    assert_eq!(forwarded_start(&herdr)["args"], json!(["--no-daemon", "resume", OLDER]));
}

#[tokio::test]
async fn a_session_without_a_transcript_is_not_started() {
    let home = claude_home();
    let herdr = plain_herdr().await;
    let (_service, mut client) = connected(&herdr, &home).await;

    let reply = client
        .request(json!({
            "type": "agent.start", "request_id": "r2", "pane_id": "w9:p1", "name": "garden-missing", "kind": "claude",
            "resume_session": "00000000-0000-0000-0000-00000000dead",
        }))
        .await;

    assert_eq!(reply["code"], "transcript_absent");
    assert_eq!(herdr.methods(), ["ping"]);
}

#[tokio::test]
async fn unsafe_ids_and_other_agents_cannot_resume() {
    for (kind, session) in [
        ("claude", "../../etc/passwd"),
        ("claude", "--dangerously-skip-permissions"),
        ("claude", ""),
        ("opencode", OLDER),
    ] {
        let home = claude_home();
        let herdr = plain_herdr().await;
        let (_service, mut client) = connected(&herdr, &home).await;

        let reply = client
            .request(json!({"type": "agent.start", "request_id": "r2", "pane_id": "w9:p1", "name": "garden", "kind": kind, "resume_session": session}))
            .await;

        assert_eq!(reply["code"], "invalid_message", "{kind} {session:?}");
        assert_eq!(herdr.methods(), ["ping"], "{kind} {session:?}");
    }
}
