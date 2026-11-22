//! 新增聊天與 workspace／worktree 操作(raychiutw/purrdr.host#15):檢查參數後轉送 herdr 本機 socket 的對應方法。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::connection::{error, herdr_error};
use crate::herdr_client::HerdrClient;
use crate::http::blocking;
use crate::pane_terminal;
use crate::protocol::{
    AgentStart, AgentStartResult, ErrorCode, ResumableAgentKind, ServerMessage, TabRename, TabRenameResult, WorkspaceClose, WorkspaceCloseResult,
    WorkspaceCreate, WorkspaceCreateResult, WorkspaceRename, WorkspaceRenameResult, WorktreeCreate, WorktreeCreateResult,
};
use crate::session_history;
use crate::transcript::TranscriptEnvironment;

/// 建立 workspace 或 worktree 時 herdr 可能要等 shell 與 git;比 app 的 60 秒期限短，app 放棄前先收到結果或錯誤。
const CREATION_WAIT: Duration = Duration::from_secs(50);

/// workspace 名稱的長度上限(字元)。
const MAX_LABEL_LENGTH: usize = 200;

/// 分支名稱的長度上限(bytes)。
const MAX_BRANCH_LENGTH: usize = 200;

/// herdr agent.start 接受的 timeout_ms(大於 3 秒、最多 5 分鐘)。
const AGENT_START_TIMEOUT_MS: std::ops::RangeInclusive<i64> = 3001..=300_000;

/// 沒帶 timeout_ms 時 herdr 等 Agent 就緒的時間。
const DEFAULT_AGENT_START_TIMEOUT_MS: u64 = 30_000;

/// 在主機上的資料夾建立 workspace,回傳 root pane。
pub async fn workspace_create(herdr: &HerdrClient, request: WorkspaceCreate) -> ServerMessage {
    if has_control_characters(&request.cwd) || !Path::new(&request.cwd).is_absolute() {
        return invalid(request.request_id, "cwd must be an absolute path");
    }
    if !is_existing_folder(&request.cwd).await {
        return error(
            Some(request.request_id),
            ErrorCode::DirectoryNotFound,
            "cwd is not an existing folder on this host",
        );
    }
    if request.label.as_deref().is_some_and(|label| !is_valid_label(label)) {
        return invalid(request.request_id, "label must not be blank or contain control characters");
    }
    let mut params = json!({"cwd": request.cwd, "focus": false});
    if let Some(label) = request.label {
        params["label"] = json!(label);
    }
    match herdr.request_waiting("workspace.create", params, CREATION_WAIT).await {
        Ok(result) => {
            let (workspace_id, root_pane_id) = created_workspace(&result);
            ServerMessage::WorkspaceCreateResult(WorkspaceCreateResult {
                request_id: request.request_id,
                workspace_id,
                root_pane_id,
            })
        }
        Err(failure) => herdr_error(request.request_id, "workspace.create", failure),
    }
}

/// 重新命名 workspace,回傳 herdr 實際保存的名稱。
pub async fn workspace_rename(herdr: &HerdrClient, request: WorkspaceRename) -> ServerMessage {
    if !is_valid_id(&request.workspace_id) || !is_valid_label(&request.label) {
        return invalid(request.request_id, "invalid workspace_id or label");
    }
    let params = json!({"workspace_id": request.workspace_id, "label": request.label});
    match herdr.request("workspace.rename", params).await {
        Ok(result) => ServerMessage::WorkspaceRenameResult(WorkspaceRenameResult {
            request_id: request.request_id,
            label: result["workspace"]["label"].as_str().map(str::to_owned),
        }),
        Err(failure) => herdr_error(request.request_id, "workspace.rename", failure),
    }
}

/// 重新命名 tab,回傳 herdr 實際保存的名稱(herdr 回的 result 沒有 tab.label 時省略)。
pub async fn tab_rename(herdr: &HerdrClient, request: TabRename) -> ServerMessage {
    if !is_valid_id(&request.tab_id) || !is_valid_label(&request.label) {
        return invalid(request.request_id, "invalid tab_id or label");
    }
    let params = json!({"tab_id": request.tab_id, "label": request.label});
    match herdr.request("tab.rename", params).await {
        Ok(result) => ServerMessage::TabRenameResult(TabRenameResult {
            request_id: request.request_id,
            label: result["tab"]["label"].as_str().map(str::to_owned),
        }),
        Err(failure) => herdr_error(request.request_id, "tab.rename", failure),
    }
}

/// 關閉 workspace(不連同 worktree 群組)。
pub async fn workspace_close(herdr: &HerdrClient, request: WorkspaceClose) -> ServerMessage {
    if !is_valid_id(&request.workspace_id) {
        return invalid(request.request_id, "invalid workspace_id");
    }
    match herdr.request("workspace.close", json!({"workspace_id": request.workspace_id})).await {
        Ok(_) => ServerMessage::WorkspaceCloseResult(WorkspaceCloseResult {
            request_id: request.request_id,
        }),
        Err(failure) => herdr_error(request.request_id, "workspace.close", failure),
    }
}

/// 以 workspace 的 repo 建立 worktree 並開成新的 workspace。
pub async fn worktree_create(herdr: &HerdrClient, request: WorktreeCreate) -> ServerMessage {
    if !is_valid_id(&request.workspace_id) || !is_valid_branch(&request.branch) {
        return invalid(request.request_id, "invalid workspace_id or branch");
    }
    let params = json!({"workspace_id": request.workspace_id, "branch": request.branch, "focus": false});
    match herdr.request_waiting("worktree.create", params, CREATION_WAIT).await {
        Ok(result) => {
            let (workspace_id, root_pane_id) = created_workspace(&result);
            ServerMessage::WorktreeCreateResult(WorktreeCreateResult {
                request_id: request.request_id,
                workspace_id,
                root_pane_id,
            })
        }
        Err(failure) => herdr_error(request.request_id, "worktree.create", failure),
    }
}

/// 在 pane 的 shell 啟動 Agent。
pub async fn agent_start(herdr: &HerdrClient, request: AgentStart, transcripts: &TranscriptEnvironment) -> ServerMessage {
    let timeout_is_valid = request.timeout_ms.is_none_or(|timeout| AGENT_START_TIMEOUT_MS.contains(&timeout));
    if !is_valid_id(&request.pane_id) || !is_valid_id(&request.name) || !is_valid_agent_kind(&request.kind) || !timeout_is_valid {
        return invalid(request.request_id, "invalid pane_id, name, kind or timeout_ms");
    }
    if request.permission_mode.is_some() && request.kind != "claude" {
        return invalid(request.request_id, "permission_mode is only for claude");
    }
    let resume = match request.resume_session.as_deref().map(|session| resume_kind(&request.kind, session)) {
        None => None,
        Some(Some(resume)) => Some(resume),
        Some(None) => return invalid(request.request_id, "resume_session needs a claude or codex session id (letters, digits and -)"),
    };
    if let Some((kind, session)) = resume.clone() {
        let environment = transcripts.clone();
        if !blocking(move || session_history::exists(&environment, kind, &session)).await {
            return error(
                Some(request.request_id),
                ErrorCode::TranscriptAbsent,
                "no transcript for resume_session on this host",
            );
        }
    }
    let mut params = json!({"name": request.name, "kind": request.kind, "pane_id": request.pane_id});
    let mut args = Vec::new();
    if let Some(permission_mode) = request.permission_mode {
        args.extend([json!("--permission-mode"), json!(permission_mode)]);
    }
    match resume {
        Some((ResumableAgentKind::Claude, session)) => args.extend([json!("--resume"), json!(session)]),
        Some((ResumableAgentKind::Codex, session)) => args.extend([json!("resume"), json!(session)]),
        None => {}
    }
    if !args.is_empty() {
        params["args"] = json!(args);
    }
    let mut herdr_wait = Duration::from_millis(DEFAULT_AGENT_START_TIMEOUT_MS);
    if let Some(timeout) = request.timeout_ms {
        params["timeout_ms"] = json!(timeout);
        herdr_wait = Duration::from_millis(timeout.unsigned_abs());
    }
    // 絕不比 herdr 先放棄：否則 app 重試會在同一個 pane 再啟動一個 Agent。
    match herdr.request_waiting("agent.start", params, herdr_wait + CREATION_WAIT).await {
        Ok(_) => ServerMessage::AgentStartResult(AgentStartResult {
            request_id: request.request_id,
        }),
        Err(failure) => herdr_error(request.request_id, "agent.start", failure),
    }
}

/// 可以續接的 Agent 與安全的 session id;不行時 None。
fn resume_kind(kind: &str, session: &str) -> Option<(ResumableAgentKind, String)> {
    let kind = match kind {
        "claude" => ResumableAgentKind::Claude,
        "codex" => ResumableAgentKind::Codex,
        _ => return None,
    };
    session_history::is_safe_id(session).then(|| (kind, session.to_owned()))
}

/// herdr 以 kind 當執行檔名稱，只收小寫英數字。
fn is_valid_agent_kind(kind: &str) -> bool {
    !kind.is_empty() && kind.len() <= 32 && kind.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

/// 擋掉會被當成選項或 git 不接受的名稱;其餘交給 git 判斷。
fn is_valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= MAX_BRANCH_LENGTH
        && !branch.starts_with('-')
        && !branch.contains("..")
        && !branch
            .chars()
            .any(|character| character.is_whitespace() || character.is_control() || matches!(character, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
}

/// herdr 的 workspace／pane id:規則同 Pane 終端機的 target。
fn is_valid_id(id: &str) -> bool {
    pane_terminal::is_valid_target(id)
}

/// herdr 遇到不存在的資料夾會改開家目錄，所以先確認。
async fn is_existing_folder(cwd: &str) -> bool {
    let folder = PathBuf::from(cwd);
    blocking(move || folder.is_dir()).await
}

fn is_valid_label(label: &str) -> bool {
    !label.trim().is_empty() && label.chars().count() <= MAX_LABEL_LENGTH && !has_control_characters(label)
}

fn has_control_characters(text: &str) -> bool {
    text.chars().any(char::is_control)
}

fn invalid(request_id: String, message: &str) -> ServerMessage {
    error(Some(request_id), ErrorCode::InvalidMessage, message)
}

/// `workspace_created` 與 `worktree_created` 共用的 workspace id 與 root pane id。
fn created_workspace(result: &Value) -> (String, String) {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    (text(&result["workspace"]["workspace_id"]), text(&result["root_pane"]["pane_id"]))
}
