//! 新增聊天與 workspace／worktree 操作(raychiutw/purrdr.host#15):轉送 herdr 對應方法，參數不合格不送到 herdr。

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, recorded, start_service};
use tempfile::TempDir;

fn hello(request_id: &str) -> Value {
    json!({"type": "hello", "request_id": request_id, "protocol_version": 1})
}

/// herdr 0.9.5 在這個平台錄的回應:Windows 回放 Windows 主機的，Unix 回放 Linux(WSL)的 POSIX 路徑版。
fn platform_recorded(name: &str) -> Value {
    if cfg!(windows) {
        recorded(&format!("windows/{name}"))
    } else {
        recorded(name)
    }
}

async fn herdr_with(replies: Vec<(&str, Value)>) -> FakeHerdr {
    let mut all_replies = vec![("ping", recorded("ping.ndjson"))];
    all_replies.extend(replies);
    FakeHerdr::start(all_replies).await
}

fn forwarded(herdr: &FakeHerdr, method: &str) -> Value {
    herdr
        .requests()
        .into_iter()
        .find(|request| request["method"] == method)
        .unwrap_or_else(|| panic!("沒有送出 {method}"))["params"]
        .clone()
}

fn path_text(directory: &TempDir) -> String {
    directory.path().to_string_lossy().into_owned()
}

#[tokio::test]
async fn workspace_create_is_forwarded_without_focus_and_returns_its_root_pane() {
    let herdr = herdr_with(vec![("workspace.create", platform_recorded("workspace.create.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;
    let folder = TempDir::new().unwrap();

    let reply = client
        .request(json!({"type": "workspace.create", "request_id": "r2", "cwd": path_text(&folder), "label": "garden-journal"}))
        .await;

    assert_eq!(
        reply,
        json!({"type": "workspace.create.result", "request_id": "r2", "workspace_id": "w1", "root_pane_id": "w1:p1"})
    );
    assert_eq!(
        forwarded(&herdr, "workspace.create"),
        json!({"cwd": path_text(&folder), "label": "garden-journal", "focus": false})
    );
}

async fn rejected_without_reaching_herdr(request: Value, method: &str) {
    rejected_with_code_without_reaching_herdr(request, method, "invalid_message").await;
}

async fn rejected_with_code_without_reaching_herdr(request: Value, method: &str, code: &str) {
    let herdr = herdr_with(vec![(method, json!({"id": "", "result": {"type": "ok"}}))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client.request(request.clone()).await;

    assert_eq!(reply["code"], code, "{request}");
    assert_eq!(reply["request_id"], request["request_id"]);
    assert_eq!(herdr.methods(), ["ping"], "{request}");
}

#[tokio::test]
async fn workspace_create_rejects_relative_folders() {
    for cwd in ["Projects/garden-journal", ""] {
        rejected_without_reaching_herdr(json!({"type": "workspace.create", "request_id": "r2", "cwd": cwd}), "workspace.create").await;
    }
}

#[tokio::test]
async fn workspace_create_reports_missing_folders_and_files_as_directory_not_found() {
    let folder = TempDir::new().unwrap();
    let missing = folder.path().join("missing").to_string_lossy().into_owned();
    let file = folder.path().join("plain.txt");
    std::fs::write(&file, "x").unwrap();
    for cwd in [missing, file.to_string_lossy().into_owned()] {
        rejected_with_code_without_reaching_herdr(
            json!({"type": "workspace.create", "request_id": "r2", "cwd": cwd}),
            "workspace.create",
            "directory_not_found",
        )
        .await;
    }
}

#[tokio::test]
async fn workspace_create_rejects_blank_labels() {
    let folder = TempDir::new().unwrap();
    rejected_without_reaching_herdr(
        json!({"type": "workspace.create", "request_id": "r2", "cwd": path_text(&folder), "label": "  "}),
        "workspace.create",
    )
    .await;
}

#[tokio::test]
async fn workspace_rename_is_forwarded_and_returns_the_label_herdr_kept() {
    let herdr = herdr_with(vec![("workspace.rename", platform_recorded("workspace.rename.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "workspace.rename", "request_id": "r2", "workspace_id": "w1", "label": "garden-notes"}))
        .await;

    assert_eq!(reply, json!({"type": "workspace.rename.result", "request_id": "r2", "label": "garden-notes"}));
    assert_eq!(forwarded(&herdr, "workspace.rename"), json!({"workspace_id": "w1", "label": "garden-notes"}));
}

#[tokio::test]
async fn tab_rename_is_forwarded_and_returns_the_label_herdr_kept() {
    let herdr = herdr_with(vec![("tab.rename", platform_recorded("tab.rename.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "tab.rename", "request_id": "r2", "tab_id": "w1:t1", "label": "build-logs"}))
        .await;

    assert_eq!(reply, json!({"type": "tab.rename.result", "request_id": "r2", "label": "build-logs"}));
    assert_eq!(forwarded(&herdr, "tab.rename"), json!({"tab_id": "w1:t1", "label": "build-logs"}));
}

#[tokio::test]
async fn tab_rename_rejects_unsafe_ids_and_blank_labels() {
    for (tab_id, label) in [("-t1", "logs"), ("t1;rm", "logs"), ("", "logs"), ("w1:t1", "")] {
        rejected_without_reaching_herdr(
            json!({"type": "tab.rename", "request_id": "r2", "tab_id": tab_id, "label": label}),
            "tab.rename",
        )
        .await;
    }
}

#[tokio::test]
async fn tab_rename_of_missing_tab_keeps_herdr_error_code() {
    let herdr = herdr_with(vec![("tab.rename", platform_recorded("error.tab-rename-not-found.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "tab.rename", "request_id": "r2", "tab_id": "w9:t9", "label": "x"}))
        .await;

    assert_eq!(reply["code"], "herdr_error");
    assert_eq!(reply["herdr_code"], "tab_not_found");
}

#[tokio::test]
async fn workspace_rename_rejects_unsafe_ids_and_blank_labels() {
    for (workspace_id, label) in [("-w1", "garden"), ("w1;rm", "garden"), ("", "garden"), ("w1", "")] {
        rejected_without_reaching_herdr(
            json!({"type": "workspace.rename", "request_id": "r2", "workspace_id": workspace_id, "label": label}),
            "workspace.rename",
        )
        .await;
    }
}

#[tokio::test]
async fn workspace_rename_of_missing_workspace_keeps_herdr_error_code() {
    let herdr = herdr_with(vec![("workspace.rename", platform_recorded("error.workspace-rename-not-found.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "workspace.rename", "request_id": "r2", "workspace_id": "w99", "label": "x"}))
        .await;

    assert_eq!(reply["code"], "herdr_error");
    assert_eq!(reply["herdr_code"], "workspace_not_found");
}

#[tokio::test]
async fn workspace_close_is_forwarded_without_closing_the_worktree_group() {
    let herdr = herdr_with(vec![("workspace.close", platform_recorded("workspace.close.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "workspace.close", "request_id": "r2", "workspace_id": "w2"}))
        .await;

    assert_eq!(reply, json!({"type": "workspace.close.result", "request_id": "r2"}));
    assert_eq!(forwarded(&herdr, "workspace.close"), json!({"workspace_id": "w2"}));
}

#[tokio::test]
async fn workspace_close_with_open_worktrees_keeps_herdr_error_code() {
    let herdr = herdr_with(vec![("workspace.close", platform_recorded("error.workspace-close-group-required.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "workspace.close", "request_id": "r2", "workspace_id": "w1"}))
        .await;

    assert_eq!(reply["code"], "herdr_error");
    assert_eq!(reply["herdr_code"], "workspace_group_close_required");
}

#[tokio::test]
async fn workspace_close_rejects_unsafe_ids() {
    for workspace_id in ["-w1", "w1 w2", ""] {
        rejected_without_reaching_herdr(
            json!({"type": "workspace.close", "request_id": "r2", "workspace_id": workspace_id}),
            "workspace.close",
        )
        .await;
    }
}

#[tokio::test]
async fn worktree_create_is_forwarded_without_focus_and_returns_the_new_workspace() {
    let herdr = herdr_with(vec![("worktree.create", platform_recorded("worktree.create.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "worktree.create", "request_id": "r2", "workspace_id": "w1", "branch": "feature/sketch"}))
        .await;

    assert_eq!(
        reply,
        json!({"type": "worktree.create.result", "request_id": "r2", "workspace_id": "w3", "root_pane_id": "w3:p1"})
    );
    assert_eq!(
        forwarded(&herdr, "worktree.create"),
        json!({"workspace_id": "w1", "branch": "feature/sketch", "focus": false})
    );
}

#[tokio::test]
async fn worktree_create_outside_a_git_repo_keeps_herdr_error_code() {
    let herdr = herdr_with(vec![("worktree.create", recorded("error.not-git-worktree.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client
        .request(json!({"type": "worktree.create", "request_id": "r2", "workspace_id": "w2", "branch": "feature/sketch"}))
        .await;

    assert_eq!(reply["code"], "herdr_error");
    assert_eq!(reply["herdr_code"], "not_git_worktree");
}

#[tokio::test]
async fn worktree_create_rejects_unsafe_ids_and_branch_names() {
    for (workspace_id, branch) in [
        ("-w1", "feature/sketch"),
        ("w1", "-b"),
        ("w1", "feature sketch"),
        ("w1", "feature/../main"),
        ("w1", ""),
        ("w1", "main\n"),
    ] {
        rejected_without_reaching_herdr(
            json!({"type": "worktree.create", "request_id": "r2", "workspace_id": workspace_id, "branch": branch}),
            "worktree.create",
        )
        .await;
    }
}

fn agent_start(request_id: &str) -> Value {
    json!({
        "type": "agent.start",
        "request_id": request_id,
        "pane_id": "w3:p1",
        "name": "feature-sketch",
        "kind": "claude",
        "permission_mode": "acceptEdits",
        "timeout_ms": 60000
    })
}

#[tokio::test]
async fn agent_start_is_forwarded_with_the_claude_permission_mode_as_arguments() {
    let herdr = herdr_with(vec![("agent.start", platform_recorded("agent.start.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client.request(agent_start("r2")).await;

    assert_eq!(reply, json!({"type": "agent.start.result", "request_id": "r2"}));
    assert_eq!(
        forwarded(&herdr, "agent.start"),
        json!({
            "name": "feature-sketch",
            "kind": "claude",
            "pane_id": "w3:p1",
            "args": ["--permission-mode", "acceptEdits"],
            "timeout_ms": 60000
        })
    );
}

#[tokio::test]
async fn agent_start_without_options_leaves_them_to_herdr() {
    let herdr = herdr_with(vec![("agent.start", platform_recorded("agent.start.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    client
        .request(json!({"type": "agent.start", "request_id": "r2", "pane_id": "w3:p1", "name": "feature-sketch", "kind": "codex"}))
        .await;

    assert_eq!(
        forwarded(&herdr, "agent.start"),
        json!({"name": "feature-sketch", "kind": "codex", "pane_id": "w3:p1"})
    );
}

#[tokio::test]
async fn agent_start_rejects_unsafe_values_and_permission_modes_for_other_agents() {
    let base = agent_start("r2");
    let with = |field: &str, value: Value| {
        let mut request = base.clone();
        request[field] = value;
        request
    };
    for request in [
        with("pane_id", json!("-w3")),
        with("pane_id", json!("w3:p1;ls")),
        with("name", json!("--resume")),
        with("name", json!("feature sketch")),
        with("name", json!("")),
        with("kind", json!("claude --yolo")),
        with("kind", json!("../claude")),
        with("kind", json!("codex")),
        with("timeout_ms", json!(3000)),
        with("timeout_ms", json!(300001)),
    ] {
        rejected_without_reaching_herdr(request, "agent.start").await;
    }
}

#[tokio::test]
async fn agent_start_on_a_busy_pane_keeps_herdr_error_code() {
    let herdr = herdr_with(vec![("agent.start", platform_recorded("error.agent-start-pane-busy.ndjson"))]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    let reply = client.request(agent_start("r2")).await;

    assert_eq!(reply["code"], "herdr_error");
    assert_eq!(reply["herdr_code"], "agent_pane_busy");
}

#[tokio::test]
async fn agent_start_waits_for_herdr_longer_than_a_plain_request() {
    let herdr = FakeHerdr::start_delayed(vec![
        ("ping", recorded("ping.ndjson"), Duration::ZERO),
        ("agent.start", platform_recorded("agent.start.ndjson"), Duration::from_secs(11)),
    ])
    .await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello("r1")).await;

    client.send(agent_start("r2")).await;
    let reply = client.receive_within(Duration::from_secs(25)).await;

    assert_eq!(reply["type"], "agent.start.result");
}

fn every_operation(folder: &TempDir) -> Vec<Value> {
    vec![
        json!({"type": "workspace.create", "request_id": "r2", "cwd": path_text(folder)}),
        json!({"type": "workspace.rename", "request_id": "r3", "workspace_id": "w1", "label": "garden-notes"}),
        json!({"type": "tab.rename", "request_id": "r7", "tab_id": "w1:t1", "label": "build-logs"}),
        json!({"type": "workspace.close", "request_id": "r4", "workspace_id": "w2"}),
        json!({"type": "worktree.create", "request_id": "r5", "workspace_id": "w1", "branch": "feature/sketch"}),
        agent_start("r6"),
    ]
}

#[tokio::test]
async fn operations_before_handshake_require_handshake() {
    let herdr = herdr_with(vec![]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    let folder = TempDir::new().unwrap();

    for request in every_operation(&folder) {
        let reply = client.request(request.clone()).await;
        assert_eq!(reply["code"], "handshake_required", "{request}");
    }
    assert!(herdr.methods().is_empty());
}

#[tokio::test]
async fn read_only_device_is_refused_and_audited_with_the_target_workspace_or_pane() {
    let herdr = herdr_with(vec![]).await;
    let service = start_service(herdr.socket.clone()).await;
    let read_only = service.pair_device("借出的 iPad", true);
    let mut client = TestClient::connect_with_token(&service, Some(&read_only.device_token)).await;
    client.request(hello("r1")).await;
    let folder = TempDir::new().unwrap();

    for request in every_operation(&folder) {
        let reply = client.request(request.clone()).await;
        assert_eq!(reply["code"], "read_only_device", "{request}");
    }

    assert_eq!(herdr.methods(), ["ping"]);
    let audited: Vec<Value> = service
        .audit_log()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["event"] == "write")
        .map(|event| json!([event["operation"], event["workspace_id"], event["pane_id"], event["allowed"]]))
        .collect();
    assert_eq!(
        audited,
        [
            json!(["workspace.create", null, null, false]),
            json!(["workspace.rename", "w1", null, false]),
            json!(["tab.rename", null, null, false]),
            json!(["workspace.close", "w2", null, false]),
            json!(["worktree.create", "w1", null, false]),
            json!(["agent.start", null, "w3:p1", false]),
        ]
    );
}
