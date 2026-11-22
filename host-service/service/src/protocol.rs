//! 由 protocol/host-service.schema.json 產生，不要手動修改。重新產生:cd host-service && cargo run -p protocol-codegen

use serde::{Deserialize, Serialize};

/// 協定版本(schema 的 `x-protocol-version`)。
pub const PROTOCOL_VERSION: i64 = 1;

/// 主機取檔的排除清單(schema 的 `x-excluded-file-locations`)。
pub const EXCLUDED_FILE_LOCATIONS: &[&str] = &[".ssh", ".gnupg", ".aws", ".azure", ".config/gcloud", ".kube", ".docker", ".netrc", ".npmrc", ".pypirc", ".git-credentials", ".config/gh", ".cargo/credentials", ".cargo/credentials.toml", ".password-store", ".local/share/keyrings", "Library/Keychains", ".codex/auth.json", ".claude/.credentials.json", ".claude.json", ".purrdr/host-service", ".purrdr/push-*", ".env", ".env.*", ".vault-token", ".terraform.d", ".m2/settings.xml", ".git/config", ".bash_history", ".zsh_history", "ConsoleHost_history.txt", "Library/Application Support/Google/Chrome", "Library/Application Support/Microsoft Edge", "Library/Application Support/BraveSoftware", "Library/Application Support/Firefox", "Library/Cookies", "Library/Safari", ".mozilla", ".config/google-chrome", ".config/chromium", ".config/microsoft-edge", ".config/BraveSoftware", "AppData/Local/Google/Chrome/User Data", "AppData/Local/Microsoft/Edge/User Data", "AppData/Local/BraveSoftware", "AppData/Roaming/Mozilla/Firefox", "AppData/Roaming/Microsoft/Credentials", "AppData/Roaming/Microsoft/Protect"];

/// 主機取檔在工作目錄以外可放行的系統暫存目錄(schema 的 `x-temporary-file-locations`)。
pub const TEMPORARY_FILE_LOCATIONS: &[&str] = &["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"];

/// 暫存目錄底下的上傳資料夾名稱(schema 的 `x-upload-folder`;Unix 之後接 `-<uid>`)。
pub const UPLOAD_FOLDER: &str = "purrdr-uploads";

/// 主機取檔的大小上限(schema 的 `x-maximum-file-size`)。
pub const MAXIMUM_FILE_SIZE: u64 = 26214400;

/// 主機取檔一次讀取最多回傳的 bytes(schema 的 `x-maximum-file-chunk`)。
pub const MAXIMUM_FILE_CHUNK: u64 = 1048576;

/// app 送給主機服務的訊息，以 type 區分。標 x-write 的是寫入類操作，唯讀裝置送出時被拒絕(read_only_device)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "hello")]
    Hello(Hello),
    #[serde(rename = "agents.list")]
    AgentsList(AgentsList),
    #[serde(rename = "pane.send_keys")]
    PaneSendKeys(PaneSendKeys),
    #[serde(rename = "transcript.locate")]
    TranscriptLocate(TranscriptLocate),
    #[serde(rename = "transcript.window")]
    TranscriptWindow(TranscriptWindow),
    #[serde(rename = "transcript.follow")]
    TranscriptFollow(TranscriptFollow),
    #[serde(rename = "transcript.unfollow")]
    TranscriptUnfollow(TranscriptUnfollow),
    #[serde(rename = "transcript.previews")]
    TranscriptPreviews(TranscriptPreviews),
    #[serde(rename = "transcript.subagent")]
    TranscriptSubagent(TranscriptSubagent),
    #[serde(rename = "transcript.workflow")]
    TranscriptWorkflow(TranscriptWorkflow),
    #[serde(rename = "transcript.delegation")]
    TranscriptDelegation(TranscriptDelegation),
    #[serde(rename = "pane.send_text")]
    PaneSendText(PaneSendText),
    #[serde(rename = "agent.prompt")]
    AgentPrompt(AgentPrompt),
    #[serde(rename = "events.subscribe")]
    EventsSubscribe(EventsSubscribe),
    #[serde(rename = "events.unsubscribe")]
    EventsUnsubscribe(EventsUnsubscribe),
    #[serde(rename = "pane.terminal.open")]
    PaneTerminalOpen(PaneTerminalOpen),
    #[serde(rename = "pane.terminal.input")]
    PaneTerminalInput(PaneTerminalInput),
    #[serde(rename = "pane.terminal.resize")]
    PaneTerminalResize(PaneTerminalResize),
    #[serde(rename = "pane.terminal.scroll")]
    PaneTerminalScroll(PaneTerminalScroll),
    #[serde(rename = "pane.terminal.close")]
    PaneTerminalClose(PaneTerminalClose),
    #[serde(rename = "file.stat")]
    FileStat(FileStat),
    #[serde(rename = "file.read")]
    FileRead(FileRead),
    #[serde(rename = "directory.list")]
    DirectoryList(DirectoryList),
    #[serde(rename = "push.status")]
    PushStatus(PushStatus),
    #[serde(rename = "push.install")]
    PushInstall(PushInstall),
    #[serde(rename = "push.remove")]
    PushRemove(PushRemove),
    #[serde(rename = "push.update_muted")]
    PushUpdateMuted(PushUpdateMuted),
    #[serde(rename = "push.test")]
    PushTest(PushTest),
    #[serde(rename = "usage.codex")]
    UsageCodex(UsageCodex),
    #[serde(rename = "usage.claude")]
    UsageClaude(UsageClaude),
    #[serde(rename = "usage.session_context")]
    UsageSessionContext(UsageSessionContext),
    #[serde(rename = "claude_usage.status")]
    ClaudeUsageStatus(ClaudeUsageStatus),
    #[serde(rename = "claude_usage.install")]
    ClaudeUsageInstall(ClaudeUsageInstall),
    #[serde(rename = "claude_usage.remove")]
    ClaudeUsageRemove(ClaudeUsageRemove),
    #[serde(rename = "workspace.create")]
    WorkspaceCreate(WorkspaceCreate),
    #[serde(rename = "workspace.rename")]
    WorkspaceRename(WorkspaceRename),
    #[serde(rename = "workspace.close")]
    WorkspaceClose(WorkspaceClose),
    #[serde(rename = "worktree.create")]
    WorktreeCreate(WorktreeCreate),
    #[serde(rename = "agent.start")]
    AgentStart(AgentStart),
    #[serde(rename = "file.upload.start")]
    FileUploadStart(FileUploadStart),
    #[serde(rename = "file.upload.chunk")]
    FileUploadChunk(FileUploadChunk),
    #[serde(rename = "devices.list")]
    DevicesList(DevicesList),
    #[serde(rename = "devices.revoke")]
    DevicesRevoke(DevicesRevoke),
    #[serde(rename = "host_service.status")]
    HostServiceStatus(HostServiceStatus),
    #[serde(rename = "host_service.update")]
    HostServiceUpdate(HostServiceUpdate),
    #[serde(rename = "host_service.set_auto_update")]
    HostServiceSetAutoUpdate(HostServiceSetAutoUpdate),
    #[serde(rename = "pane.read")]
    PaneRead(PaneRead),
    #[serde(rename = "sessions.list")]
    SessionsList(SessionsList),
    #[serde(rename = "git.status")]
    GitStatus(GitStatus),
    #[serde(rename = "git.diff")]
    GitDiff(GitDiff),
    #[serde(rename = "queue.list")]
    QueueList(QueueList),
    #[serde(rename = "queue.add")]
    QueueAdd(QueueAdd),
    #[serde(rename = "queue.remove")]
    QueueRemove(QueueRemove),
    #[serde(rename = "queue.reorder")]
    QueueReorder(QueueReorder),
}

impl ClientMessage {
    /// 寫入類訊息(schema 的 `x-write`);唯讀裝置送出時被拒絕。
    pub fn is_write(&self) -> bool {
        matches!(self, ClientMessage::PaneSendKeys(_) | ClientMessage::PaneSendText(_) | ClientMessage::AgentPrompt(_) | ClientMessage::PaneTerminalInput(_) | ClientMessage::PaneTerminalResize(_) | ClientMessage::PaneTerminalScroll(_) | ClientMessage::PushInstall(_) | ClientMessage::PushRemove(_) | ClientMessage::PushUpdateMuted(_) | ClientMessage::PushTest(_) | ClientMessage::ClaudeUsageInstall(_) | ClientMessage::ClaudeUsageRemove(_) | ClientMessage::WorkspaceCreate(_) | ClientMessage::WorkspaceRename(_) | ClientMessage::WorkspaceClose(_) | ClientMessage::WorktreeCreate(_) | ClientMessage::AgentStart(_) | ClientMessage::FileUploadStart(_) | ClientMessage::FileUploadChunk(_) | ClientMessage::DevicesRevoke(_) | ClientMessage::HostServiceUpdate(_) | ClientMessage::HostServiceSetAutoUpdate(_) | ClientMessage::QueueAdd(_) | ClientMessage::QueueRemove(_) | ClientMessage::QueueReorder(_))
    }
}

/// 主機服務送給 app 的訊息，以 type 區分。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "welcome")]
    Welcome(Welcome),
    #[serde(rename = "agents.list.result")]
    AgentsListResult(AgentsListResult),
    #[serde(rename = "pane.send_keys.result")]
    PaneSendKeysResult(PaneSendKeysResult),
    #[serde(rename = "transcript.locate.result")]
    TranscriptLocateResult(TranscriptLocateResult),
    #[serde(rename = "transcript.window.result")]
    TranscriptWindowResult(TranscriptWindowResult),
    #[serde(rename = "transcript.follow.result")]
    TranscriptFollowResult(TranscriptFollowResult),
    #[serde(rename = "transcript.update")]
    TranscriptUpdate(TranscriptUpdate),
    #[serde(rename = "transcript.unfollow.result")]
    TranscriptUnfollowResult(TranscriptUnfollowResult),
    #[serde(rename = "transcript.previews.result")]
    TranscriptPreviewsResult(TranscriptPreviewsResult),
    #[serde(rename = "transcript.subagent.result")]
    TranscriptSubagentResult(TranscriptSubagentResult),
    #[serde(rename = "transcript.workflow.result")]
    TranscriptWorkflowResult(TranscriptWorkflowResult),
    #[serde(rename = "transcript.delegation.result")]
    TranscriptDelegationResult(TranscriptDelegationResult),
    #[serde(rename = "pane.send_text.result")]
    PaneSendTextResult(PaneSendTextResult),
    #[serde(rename = "agent.prompt.result")]
    AgentPromptResult(AgentPromptResult),
    #[serde(rename = "events.subscribe.result")]
    EventsSubscribeResult(EventsSubscribeResult),
    #[serde(rename = "events.event")]
    SubscribedEvent(SubscribedEvent),
    #[serde(rename = "events.ended")]
    EventsEnded(EventsEnded),
    #[serde(rename = "events.unsubscribe.result")]
    EventsUnsubscribeResult(EventsUnsubscribeResult),
    #[serde(rename = "pane.terminal.open.result")]
    PaneTerminalOpenResult(PaneTerminalOpenResult),
    #[serde(rename = "pane.terminal.frame")]
    PaneTerminalFrame(PaneTerminalFrame),
    #[serde(rename = "pane.terminal.closed")]
    PaneTerminalClosed(PaneTerminalClosed),
    #[serde(rename = "file.stat.result")]
    FileStatResult(FileStatResult),
    #[serde(rename = "directory.list.result")]
    DirectoryListResult(DirectoryListResult),
    #[serde(rename = "file.read.result")]
    FileReadResult(FileReadResult),
    #[serde(rename = "push.status.result")]
    PushStatusResult(PushStatusResult),
    #[serde(rename = "push.install.result")]
    PushInstallResult(PushInstallResult),
    #[serde(rename = "push.remove.result")]
    PushRemoveResult(PushRemoveResult),
    #[serde(rename = "push.update_muted.result")]
    PushUpdateMutedResult(PushUpdateMutedResult),
    #[serde(rename = "push.test.result")]
    PushTestResult(PushTestResult),
    #[serde(rename = "usage.codex.result")]
    UsageCodexResult(UsageCodexResult),
    #[serde(rename = "usage.claude.result")]
    UsageClaudeResult(UsageClaudeResult),
    #[serde(rename = "usage.session_context.result")]
    UsageSessionContextResult(UsageSessionContextResult),
    #[serde(rename = "claude_usage.status.result")]
    ClaudeUsageStatusResult(ClaudeUsageStatusResult),
    #[serde(rename = "claude_usage.install.result")]
    ClaudeUsageInstallResult(ClaudeUsageInstallResult),
    #[serde(rename = "claude_usage.remove.result")]
    ClaudeUsageRemoveResult(ClaudeUsageRemoveResult),
    #[serde(rename = "workspace.create.result")]
    WorkspaceCreateResult(WorkspaceCreateResult),
    #[serde(rename = "workspace.rename.result")]
    WorkspaceRenameResult(WorkspaceRenameResult),
    #[serde(rename = "workspace.close.result")]
    WorkspaceCloseResult(WorkspaceCloseResult),
    #[serde(rename = "worktree.create.result")]
    WorktreeCreateResult(WorktreeCreateResult),
    #[serde(rename = "agent.start.result")]
    AgentStartResult(AgentStartResult),
    #[serde(rename = "file.upload.start.result")]
    FileUploadStartResult(FileUploadStartResult),
    #[serde(rename = "file.upload.chunk.result")]
    FileUploadChunkResult(FileUploadChunkResult),
    #[serde(rename = "devices.list.result")]
    DevicesListResult(DevicesListResult),
    #[serde(rename = "devices.revoke.result")]
    DevicesRevokeResult(DevicesRevokeResult),
    #[serde(rename = "host_service.status.result")]
    HostServiceStatusResult(HostServiceStatusResult),
    #[serde(rename = "host_service.update.result")]
    HostServiceUpdateResult(HostServiceUpdateResult),
    #[serde(rename = "host_service.set_auto_update.result")]
    HostServiceSetAutoUpdateResult(HostServiceSetAutoUpdateResult),
    #[serde(rename = "error")]
    ErrorMessage(ErrorMessage),
    #[serde(rename = "pane.read.result")]
    PaneReadResult(PaneReadResult),
    #[serde(rename = "sessions.list.result")]
    SessionsListResult(SessionsListResult),
    #[serde(rename = "git.status.result")]
    GitStatusResult(GitStatusResult),
    #[serde(rename = "git.diff.result")]
    GitDiffResult(GitDiffResult),
    #[serde(rename = "queue.list.result")]
    QueueListResult(QueueListResult),
    #[serde(rename = "queue.add.result")]
    QueueAddResult(QueueAddResult),
    #[serde(rename = "queue.remove.result")]
    QueueRemoveResult(QueueRemoveResult),
    #[serde(rename = "queue.reorder.result")]
    QueueReorderResult(QueueReorderResult),
    #[serde(rename = "queue.changed")]
    QueueChanged(QueueChanged),
}

/// 握手：連線後第一個請求，帶 app 使用的協定版本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// 請求識別碼，回應帶回同一個值。
    pub request_id: String,
    pub protocol_version: i64,
    /// app 版本，只用於診斷。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

/// 握手成功：主機服務的協定版本、版本，herdr 伺服器 ping 回報的版本與能力，以及這支裝置是否唯讀。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub request_id: String,
    pub protocol_version: i64,
    pub host_service_version: String,
    pub herdr: HerdrServer,
    /// 這支裝置配對成唯讀：寫入類操作會被拒絕(read_only_device),app 停用輸入。
    pub read_only: bool,
    /// 這台主機使用者家目錄的絕對路徑(0.12.0 起);主機找不到家目錄時省略。app 用它把 ~ 換成路徑、當作 directory.list 的起點;舊版主機沒有這個欄位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_directory: Option<String>,
}

/// herdr 伺服器(不是 CLI)的版本與能力。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HerdrServer {
    pub version: String,
    pub protocol: i64,
    /// ping 的 capabilities 裡值為 true 的名稱。
    pub capabilities: Vec<String>,
}

/// 取得 Agent 列表(herdr session.snapshot)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentsList {
    pub request_id: String,
}

/// Agent 列表與它們所在的 workspace。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentsListResult {
    pub request_id: String,
    pub workspaces: Vec<Workspace>,
    pub agents: Vec<Agent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
}

/// herdr 在某個 Pane 偵測到的 Agent。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// herdr 的 agent 名稱，例如 claude、codex。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub status: AgentStatus,
    /// 去掉狀態符號的終端機標題。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<AgentSession>,
}

/// herdr 回報的 Agent 狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "working")]
    Working,
    #[serde(rename = "blocked")]
    Blocked,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "unknown")]
    Unknown,
}

/// herdr 的 agent_session:定位對話紀錄用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSession {
    pub source: String,
    pub agent: String,
    pub kind: String,
    pub value: String,
}

/// 對 Pane 送出按鍵(herdr pane.send_keys)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSendKeys {
    pub request_id: String,
    pub pane_id: String,
    /// herdr 鍵名，例如 enter、esc、ctrl+c。
    pub keys: Vec<String>,
    /// app 看到的 herdr agent_session id(raychiutw/purrdr.host#29)。帶了就先讀 session.snapshot 比對，這個 pane 的 Agent session 不同(或已沒有 Agent)時不送，回 pane_changed;沒帶時照送。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_session: Option<String>,
    /// app 看到的畫面指紋(pane.read 以 source visible、format text 回的 screen)。帶了就先讀畫面比對，不同時不送，回 pane_changed;回答選擇題時用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_screen: Option<String>,
}

/// 按鍵已送到 Pane。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSendKeysResult {
    pub request_id: String,
}

/// 開 Pane 終端機(ADR-0005):主機服務以子行程執行 herdr terminal session control;唯讀裝置改用 observe(只能看)。串流識別碼就是這個請求的 request_id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalOpen {
    pub request_id: String,
    /// pane id 或 terminal id。
    pub target: String,
    pub columns: i64,
    pub rows: i64,
    /// 接管別人正在操作的 Pane(唯讀裝置忽略)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takeover: Option<bool>,
}

/// Pane 終端機已開啟;畫面更新接著以 pane.terminal.frame 送出，結束時送 pane.terminal.closed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalOpenResult {
    pub request_id: String,
    /// 等於開啟請求的 request_id。
    pub stream_id: String,
    /// 唯讀裝置以 observe 接上：只能看，輸入會被拒絕。
    pub read_only: bool,
}

/// 一次畫面更新:herdr terminal.frame 原樣轉接。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalFrame {
    pub stream_id: String,
    /// 每條連線從 1 起算。
    pub seq: i64,
    pub width: i64,
    pub height: i64,
    /// true 是完整重繪。
    pub full: bool,
    /// ANSI 畫面 bytes 的標準 base64。
    pub bytes: String,
}

/// Pane 終端機結束，之後這個串流不再有訊息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalClosed {
    pub stream_id: String,
    pub ending: PaneTerminalEnding,
    /// ending 是 closed 時,herdr terminal.closed 的原文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// ending 是 failed 時,herdr 的 exit code。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// ending 是 failed 時,herdr 的 stderr(給開發者看)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// closed:herdr 送了 terminal.closed;disconnected:沒有 terminal.closed 就斷了(可重新連線);failed:herdr 沒跑起來。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneTerminalEnding {
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "disconnected")]
    Disconnected,
    #[serde(rename = "failed")]
    Failed,
}

/// Pane 終端機的鍵盤輸入(herdr terminal.input)。不回應;稽核只在開啟時記一筆，不記內容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalInput {
    pub stream_id: String,
    /// 輸入 bytes 的標準 base64,每則不超過 64 KiB。
    pub bytes: String,
}

/// 改變 viewport 格數(herdr terminal.resize)。不回應。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalResize {
    pub stream_id: String,
    pub columns: i64,
    pub rows: i64,
}

/// 捲動(herdr terminal.scroll)。source 沒帶時是 wheel。不回應。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalScroll {
    pub stream_id: String,
    pub direction: TerminalScrollDirection,
    pub lines: i64,
    pub column: i64,
    pub row: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<TerminalScrollSource>,
}

/// 捲動方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TerminalScrollDirection {
    #[serde(rename = "up")]
    Up,
    #[serde(rename = "down")]
    Down,
}

/// 捲動來源:wheel 是滾輪或單指滑動;page_key 是實體鍵盤 Page Up／Down,herdr 依 Pane 狀態捲主機 scrollback 或送 Page 鍵給程式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TerminalScrollSource {
    #[serde(rename = "wheel")]
    Wheel,
    #[serde(rename = "page_key")]
    PageKey,
}

/// 關閉 Pane 終端機:control 關閉 stdin(等同 release),observe 結束子行程。不回應。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneTerminalClose {
    pub stream_id: String,
}

/// 定位對話紀錄(#59)。session 必須是 herdr snapshot 目前某個 Agent 的 agent_session;格式與 cwd 由主機服務從 snapshot 取得，app 不能指定路徑。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptLocate {
    pub request_id: String,
    pub session: SessionReference,
    /// 是否一併回傳檔案前兩行(Codex、Pi／OMP 的 header 檢查)。
    pub header: bool,
}

/// herdr agent_session 的 kind 與 value。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReference {
    /// id 或 path。
    pub kind: String,
    pub value: String,
}

/// 對話紀錄的路徑與大小(byte)。之後這條連線才能以這個路徑讀視窗與追蹤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptLocateResult {
    pub request_id: String,
    pub path: String,
    pub size: i64,
    /// 請求 header 時，檔案前兩行(原樣)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<Vec<String>>,
}

/// 讀以 end 為終點的最後 lines 行完整的行(開啟與往上翻頁)。path 必須是這條連線定位過的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWindow {
    pub request_id: String,
    pub path: String,
    pub end: i64,
    pub lines: i64,
    /// byte 上限(raychiutw/purrdr.host#52,0.11.0 起;app 打開聊天的短載入):只取 end 往前 max_bytes bytes 內完整的行，不放大範圍，所以回應的行數可能少於 lines;最後一行就比上限大時範圍是空的(start == end,由 app 決定要不要放寬)。同 SSH 視窗腳本的第五個參數。沒帶時照舊只依行數。0 或負數回 invalid_message。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<i64>,
}

/// 視窗的 byte 範圍與範圍內的行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWindowResult {
    pub request_id: String,
    pub start: i64,
    pub end: i64,
    pub records: Vec<TranscriptRecord>,
}

/// 對話紀錄的一行：原始 byte 長度(不含換行)與剝除圖片後的內容(行為規格 R-B04、R-B06)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptRecord {
    pub length: i64,
    pub text: String,
}

/// 開始追蹤對話紀錄尾端：從 from 起，主機服務把新增的完整行(每則最多 limit bytes,落後很多時分段接著送)以 transcript.update 推送，直到 transcript.unfollow、追蹤結束或連線關閉。stream_id 由 app 指定，同一條連線內不可重複。path 必須是這條連線定位過的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptFollow {
    pub request_id: String,
    pub stream_id: String,
    pub path: String,
    pub from: i64,
    /// 每則回應或推送的上限(byte,#173):落後超過時從游標分段送出，每次最多 limit bytes 的完整行，後面還有就下一次接著送;第一行就超過 limit 時只送那一行，剝除圖片後的內容截短到 limit bytes(length 照原始長度，游標照樣前進)。#173 之前的主機服務落後超過 limit 就以 too_far_behind 結束。
    pub limit: i64,
}

/// 追蹤開始，帶目前的大小與 from 之後已有的完整行;ended 有值時主機服務沒有開始追蹤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptFollowResult {
    pub request_id: String,
    pub stream_id: String,
    pub size: i64,
    /// records 第一行的位置(byte)。
    pub start: i64,
    pub records: Vec<TranscriptRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<TranscriptStreamEnd>,
}

/// 追蹤中的對話紀錄有了新的完整行，或追蹤結束(ended)。主機服務主動推送，沒有 request_id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptUpdate {
    pub stream_id: String,
    pub size: i64,
    /// records 第一行的位置(byte)。
    pub start: i64,
    pub records: Vec<TranscriptRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<TranscriptStreamEnd>,
}

/// 主機服務停止追蹤的原因:shrank 檔案變小(換新)、replaced 檔案被換成同長或更長的新檔(游標前的內容變了)、absent 檔案不見了、unreadable 讀不到。too_far_behind 只有 #173 之前的主機服務會送(落後超過 limit;現在改為分段送出),app 仍要能接受。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TranscriptStreamEnd {
    #[serde(rename = "shrank")]
    Shrank,
    #[serde(rename = "replaced")]
    Replaced,
    #[serde(rename = "too_far_behind")]
    TooFarBehind,
    #[serde(rename = "absent")]
    Absent,
    #[serde(rename = "unreadable")]
    Unreadable,
}

/// 停止追蹤(對話關閉)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptUnfollow {
    pub request_id: String,
    pub stream_id: String,
}

/// 已停止追蹤;之後不再推送這個 stream_id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptUnfollowResult {
    pub request_id: String,
}

/// 列表預覽(raychiutw/purrdr.host#17):一次讀多個對話紀錄的檔尾。path 必須是這條連線定位過的，任一個不是就整個請求回 transcript_not_allowed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPreviews {
    pub request_id: String,
    pub entries: Vec<TranscriptPreviewEntry>,
}

/// 一個要讀預覽的對話紀錄。known_version 是上次這個主機服務回的 version(app 原樣帶回，不解讀)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPreviewEntry {
    pub key: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_version: Option<String>,
}

/// 每個讀得到的對話紀錄一項(讀不到的不在結果裡，同 SSH 的預覽腳本)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPreviewsResult {
    pub request_id: String,
    pub previews: Vec<TranscriptPreview>,
}

/// 一個對話紀錄的預覽。records 同其他對話紀錄讀取，每行是原始長度與剝除圖片後的內容:whole 是最後 48000 bytes(第一行可能被切在中間，長度從切點算起)、appended 是 known_version 之後附加的部分、unchanged 沒有內容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPreview {
    pub key: String,
    pub change: TranscriptPreviewChange,
    pub version: String,
    pub records: Vec<TranscriptRecord>,
}

/// 這次讀到的範圍:unchanged 與 known_version 相同、appended 只有附加的部分、whole 整段檔尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TranscriptPreviewChange {
    #[serde(rename = "unchanged")]
    Unchanged,
    #[serde(rename = "appended")]
    Appended,
    #[serde(rename = "whole")]
    Whole,
}

/// 找呼叫 call_id 啟動的 subagent 對話紀錄(raychiutw/purrdr.host#17):在 path 的 session 資料夾的 subagents/ 下找 toolUseId 完全等於 call_id 的 agent-*.meta.json。path 必須是這條連線可讀的對話紀錄。沒有 meta 回 transcript_absent,不只一個回 transcript_ambiguous。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSubagent {
    pub request_id: String,
    pub path: String,
    pub call_id: String,
}

/// subagent 對話紀錄的路徑。session 資料夾 subagents/ 底下的 .jsonl 都可以讀視窗與追蹤。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSubagentResult {
    pub request_id: String,
    pub path: String,
}

/// 讀 workflow 進度(raychiutw/purrdr.host#17):path 的 session 資料夾的 workflows/<run_id>.json(結束才寫)優先，否則 subagents/workflows/<run_id>/journal.jsonl。兩個都沒有回 transcript_absent。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWorkflow {
    pub request_id: String,
    pub path: String,
    pub run_id: String,
    /// 上次回的 checksum;相同時不回內容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_checksum: Option<String>,
}

/// kind 是讀到哪個檔(run 或 journal);content 是檔案原樣內容，checksum 等於 known_checksum 時省略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWorkflowResult {
    pub request_id: String,
    pub kind: TranscriptWorkflowKind,
    pub checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// run:workflows/<run_id>.json;journal:subagents/workflows/<run_id>/journal.jsonl。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TranscriptWorkflowKind {
    #[serde(rename = "run")]
    Run,
    #[serde(rename = "journal")]
    Journal,
}

/// 父對話紀錄中含 call_id 或 agent_id 的行(委派是否結束，raychiutw/purrdr.host#17)。path 必須是這條連線可讀的對話紀錄。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDelegation {
    pub request_id: String,
    pub path: String,
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// 上次回的 checksum;相同時不回內容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_checksum: Option<String>,
}

/// 這些行(剝除圖片後)的 checksum 與行;checksum 等於 known_checksum 時省略 records。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDelegationResult {
    pub request_id: String,
    pub checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<Vec<TranscriptRecord>>,
}

/// 對 Pane 送出文字(herdr pane.send_text),不補 Enter。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSendText {
    pub request_id: String,
    pub pane_id: String,
    pub text: String,
    /// app 看到的 herdr agent_session id(raychiutw/purrdr.host#29)。帶了就先讀 session.snapshot 比對，這個 pane 的 Agent session 不同(或已沒有 Agent)時不送，回 pane_changed;沒帶時照送。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_session: Option<String>,
    /// app 看到的畫面指紋(pane.read 以 source visible、format text 回的 screen)。帶了就先讀畫面比對，不同時不送，回 pane_changed;回答選擇題時用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_screen: Option<String>,
}

/// 文字已送到 Pane。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSendTextResult {
    pub request_id: String,
}

/// 讀 Pane 畫面(herdr pane.read,raychiutw/purrdr.host#28)。主機服務只轉送並回傳文字與畫面指紋，不解析語意;pane_id 規則同 Pane 終端機的 target。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneRead {
    pub request_id: String,
    pub pane_id: String,
    pub source: PaneReadSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<PaneReadFormat>,
    /// recent／recent_unwrapped 的行數(照 herdr);沒帶時由 herdr 決定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<i64>,
}

/// 照 herdr:visible 可見區、recent 最近輸出、recent_unwrapped 不折行的最近輸出、detection herdr 判斷狀態用的區段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneReadSource {
    #[serde(rename = "visible")]
    Visible,
    #[serde(rename = "recent")]
    Recent,
    #[serde(rename = "recent_unwrapped")]
    RecentUnwrapped,
    #[serde(rename = "detection")]
    Detection,
}

/// text 去掉 ANSI(預設);ansi 保留 ANSI 跳脫序列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneReadFormat {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "ansi")]
    Ansi,
}

/// 畫面文字與指紋。screen 是 text 的 UTF-8 bytes 的 SHA-256(小寫 hex);以 source visible、format text 讀到的 screen 可作為 expected_screen。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneReadResult {
    pub request_id: String,
    pub text: String,
    pub screen: String,
    /// herdr 截短了輸出。
    pub truncated: bool,
}

/// 對 Agent 送出 prompt(herdr agent.prompt,target 是 pane_id)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPrompt {
    pub request_id: String,
    pub pane_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<PromptWait>,
    /// app 看到的 herdr agent_session id(raychiutw/purrdr.host#29)。帶了就先讀 session.snapshot 比對，這個 pane 的 Agent session 不同(或已沒有 Agent)時不送，回 pane_changed;沒帶時照送。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_session: Option<String>,
    /// app 看到的畫面指紋(pane.read 以 source visible、format text 回的 screen)。帶了就先讀畫面比對，不同時不送，回 pane_changed;回答選擇題時用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_screen: Option<String>,
}

/// herdr agent.prompt 的 wait:等 Agent 進入其中一個狀態;主機服務等 herdr 的時間隨 timeout_ms 延長。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptWait {
    pub until: Vec<AgentStatus>,
    pub timeout_ms: i64,
}

/// herdr 收下 prompt。等待逾時、Agent 在等回答等情況以 herdr_error 回報(herdr_code 如 timeout、agent_blocked)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPromptResult {
    pub request_id: String,
    /// herdr 回報的送達方式(例如 written_to_pty);沒有時省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
}

/// 在主機上的資料夾建立 workspace(herdr workspace.create,不搶桌機焦點)。cwd 必須是主機上存在的絕對路徑資料夾(herdr 遇到不存在的資料夾會改開家目錄，所以主機服務先檢查)。新增聊天 = workspace.create 後對 root pane 送 agent.start。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCreate {
    pub request_id: String,
    /// 主機上的絕對路徑。
    pub cwd: String,
    /// workspace 名稱;省略時由 herdr 以資料夾名命名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// workspace 已建立;聊天開在 root_pane_id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCreateResult {
    pub request_id: String,
    pub workspace_id: String,
    pub root_pane_id: String,
}

/// 重新命名 workspace(herdr workspace.rename)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRename {
    pub request_id: String,
    pub workspace_id: String,
    /// 新名稱，不可空白。
    pub label: String,
}

/// 已重新命名。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceRenameResult {
    pub request_id: String,
    /// herdr 實際保存的名稱(可能經正規化);沒有時省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// 關閉 workspace(herdr workspace.close):裡面的 tab、pane 與程序都會停止。不連同 worktree 群組一起關，同 repo 的 worktree workspace 還開著時以 herdr_error 回報(herdr_code workspace_group_close_required)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceClose {
    pub request_id: String,
    pub workspace_id: String,
}

/// 已關閉。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCloseResult {
    pub request_id: String,
}

/// 以 workspace 所在的 git repo 建立 worktree 與分支，開成新的 workspace(herdr worktree.create,不搶桌機焦點)。分支已存在時 herdr 直接 checkout。不在 git repo 裡時以 herdr_error 回報(herdr_code not_git_worktree)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeCreate {
    pub request_id: String,
    /// 以這個 workspace 的 repo 為底。
    pub workspace_id: String,
    /// 分支名稱：不能以 - 開頭，不含空白、控制字元或 ..。
    pub branch: String,
}

/// worktree 已建立並開成新的 workspace;聊天開在 root_pane_id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeCreateResult {
    pub request_id: String,
    pub workspace_id: String,
    pub root_pane_id: String,
}

/// 在 pane 的 shell 啟動 Agent(herdr agent.start)。新增聊天 = workspace.create 或 worktree.create 後對 root pane 送這個。不接受任意參數：只有 Claude Code 可帶 permission_mode(轉成 --permission-mode),Claude Code 與 Codex 可帶 resume_session。主機服務等 herdr 的時間隨 timeout_ms 延長;Agent 停在啟動問題、pane 不在 shell 提示字元等以 herdr_error 回報(herdr_code 如 agent_not_ready、agent_pane_busy、agent_name_taken)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStart {
    pub request_id: String,
    pub pane_id: String,
    /// herdr 的 Agent 名稱：英數字、-、_、.,不能以 - 開頭。
    pub name: String,
    /// herdr 的 Agent 種類(herdr agent start --kind,例如 claude、codex),只有小寫英文字母與數字。
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<ClaudePermissionMode>,
    /// 續接的 session id(sessions.list 的 id;英數字與 -),只限 claude(轉成 --resume)與 codex(轉成 resume 子指令)。主機服務先確認這個 session 的對話紀錄存在，不在時回 transcript_absent、不啟動(herdr 對不存在的 session 一樣回已啟動，Agent 隨即結束)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session: Option<String>,
    /// 等 Agent 就緒的時間;省略時用 herdr 的預設(30 秒)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

/// Claude Code 的 --permission-mode。省略時沿用使用者在主機上的設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClaudePermissionMode {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "bypassPermissions")]
    BypassPermissions,
}

/// Agent 已在 pane 啟動。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStartResult {
    pub request_id: String,
}

/// 列出工作資料夾過去的 Claude／Codex session(raychiutw/purrdr.host#32),給續接用。Claude 讀 <設定目錄>/projects/<cwd 轉成的資料夾名>/*.jsonl;Codex 讀 sessions/ 下 session_meta 的 cwd 等於這個資料夾的 rollout。只回 id、最後修改時間與第一則使用者訊息的摘要，依修改時間新到舊，最多 50 筆。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionsList {
    pub request_id: String,
    /// 絕對路徑。
    pub cwd: String,
    pub agent_kind: ResumableAgentKind,
}

/// 可以續接 session 的 Agent。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResumableAgentKind {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
}

/// 可續接的 session;資料夾不在時是空的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionsListResult {
    pub request_id: String,
    pub sessions: Vec<HistorySession>,
}

/// 唯讀的 git 變更清單(raychiutw/purrdr.host#33):cwd 所在 repo 的工作目錄相對 HEAD 改了哪些檔案。cwd 必須是 herdr session.snapshot 當下回報的 Agent 工作目錄;以程序內函式庫讀取，不執行 git 或任何 filter 程式(所以不套用 clean／smudge 與換行轉換)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    pub request_id: String,
    pub cwd: String,
}

/// 變更的檔案，依路徑排序。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitStatusResult {
    pub request_id: String,
    /// 目前分支的短名稱;detached HEAD 時省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub files: Vec<GitFileChange>,
}

/// 一個變更的檔案;path 相對 repo 根目錄，以 / 分隔。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitFileChange {
    pub path: String,
    pub change: GitChangeKind,
}

/// 相對 HEAD:added 已加入 index 的新檔、modified 內容不同、deleted 工作目錄沒有了、untracked 沒加入 index 也沒被 .gitignore 排除的新檔。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GitChangeKind {
    #[serde(rename = "added")]
    Added,
    #[serde(rename = "modified")]
    Modified,
    #[serde(rename = "deleted")]
    Deleted,
    #[serde(rename = "untracked")]
    Untracked,
}

/// 唯讀的 git diff(工作目錄相對 HEAD 的 unified diff,前後 3 行)。cwd 規則同 git.status;帶 path 時只回這個檔案(必須是變更清單裡的路徑)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitDiff {
    pub request_id: String,
    pub cwd: String,
    /// 相對 repo 根目錄的路徑，以 / 分隔。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// 每個變更檔案的 diff。所有 diff 文字合計超過 x-maximum-file-size 時，之後的檔案只列路徑並標 truncated。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitDiffResult {
    pub request_id: String,
    pub files: Vec<GitFileDiff>,
    pub truncated: bool,
}

/// 一個檔案的 diff;沒有 diff 文字時 omitted 說明原因。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitFileDiff {
    pub path: String,
    pub change: GitChangeKind,
    /// unified diff 的 hunk(@@ 開頭，不含 ---／+++ 檔頭)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<GitDiffOmitted>,
}

/// binary 二進位檔(含 NUL 或不是 UTF-8);too_large 超過 x-maximum-file-size 或總量已達上限;excluded 路徑在 x-excluded-file-locations 排除清單。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GitDiffOmitted {
    #[serde(rename = "binary")]
    Binary,
    #[serde(rename = "too_large")]
    TooLarge,
    #[serde(rename = "excluded")]
    Excluded,
}

/// Send When Done 佇列(raychiutw/purrdr.host#30):主機服務的 Agent 觀察者在 Agent 從 working 變成 idle／done 時送出佇列的第一則(經 expected_session 比對)。帶 pane_id 時只回這個 pane。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueList {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

/// 有排隊訊息的 pane 的佇列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueListResult {
    pub request_id: String,
    pub queues: Vec<PaneQueue>,
}

/// 一個 pane 的佇列，依送出順序。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneQueue {
    pub pane_id: String,
    pub items: Vec<QueuedPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<QueuePause>,
}

/// 排隊的一則訊息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedPrompt {
    pub item_id: String,
    pub text: String,
    pub expected_session: String,
}

/// 佇列暫停的原因。session_changed:要送出時 pane 的 Agent session 已不是第一則的 expected_session,沒有送出;remove 或 reorder 換掉第一則後解除(add 加在後面不解除),下次 Agent 做完再試。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueuePause {
    #[serde(rename = "session_changed")]
    SessionChanged,
}

/// 排一則訊息到 pane 的佇列尾端;expected_session 必填(送出前比對，對象換了就不送)。每個 pane 最多 20 則(超過回 queue_full),每則最多 64 KiB(UTF-8)。佇列存在狀態資料夾，主機服務重啟後還在;稽核不記內容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueAdd {
    pub request_id: String,
    pub pane_id: String,
    pub text: String,
    pub expected_session: String,
}

/// 已排入佇列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueAddResult {
    pub request_id: String,
    pub item_id: String,
}

/// 從佇列刪除一則;不在佇列裡回 queue_item_unknown。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueRemove {
    pub request_id: String,
    pub pane_id: String,
    pub item_id: String,
}

/// 已刪除。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueRemoveResult {
    pub request_id: String,
}

/// 重排佇列:item_ids 必須恰好是目前佇列的所有 item_id(換順序);否則回 queue_item_unknown。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueReorder {
    pub request_id: String,
    pub pane_id: String,
    pub item_ids: Vec<String>,
}

/// 已重排。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueReorderResult {
    pub request_id: String,
}

/// 某個 pane 的佇列變了(新增、刪除、重排、送出或暫停),推給所有連線。sent_item_id:這次變更是因為這則已送出。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueChanged {
    pub queue: PaneQueue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_item_id: Option<String>,
}

/// 一個過去的 session。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistorySession {
    /// session id,可直接當 agent.start 的 resume_session。
    pub id: String,
    /// 對話紀錄最後修改時間(RFC 3339 UTC)。
    pub modified: String,
    /// 第一則使用者訊息，空白合併後截到 120 字;找不到時省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// 開一條 Agent 事件串流(herdr events.subscribe)。herdr 接受後回 events.subscribe.result,之後以 events.event 推送，直到 events.ended 或 events.unsubscribe。事件只是「該重讀了」的訊號，對帳用 agents.list。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsSubscribe {
    pub request_id: String,
    /// 串流識別碼(app 自訂，同一條連線內不可重複);這條串流的訊息都帶它。
    pub stream_id: String,
    pub subscriptions: Vec<EventSubscription>,
}

/// 一筆 herdr 事件訂閱。herdr 不認得的事件或不存在的 Pane 會讓整個訂閱被拒(herdr_error)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventSubscription {
    /// herdr 的訂閱型別，例如 pane.agent_status_changed、workspace.created。
    pub event: String,
    /// pane.agent_status_changed 必填;其他型別不帶。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

/// herdr 接受了訂閱。此後的變化才會推送;之前的要靠 agents.list 對帳。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsSubscribeResult {
    pub request_id: String,
    pub stream_id: String,
}

/// herdr 推送的一個事件(沒有 request_id)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscribedEvent {
    pub stream_id: String,
    /// herdr 事件的 wire 名稱，例如 pane.agent_status_changed、workspace_closed。
    pub event: String,
    /// 事件指名 Pane 時才有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

/// 事件串流在主機端結束(沒有 request_id)。有 code 表示失敗;herdr 的 events_lost 是 code herdr_error 加 herdr_code events_lost,app 重新訂閱並對帳。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsEnded {
    pub stream_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<ErrorCode>,
    /// 給開發者看的說明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// code 是 herdr_error 時，herdr 原本的錯誤碼。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr_code: Option<String>,
}

/// 關閉一條事件串流，主機服務關掉對應的 herdr 訂閱。不認得的串流識別碼也回成功。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsUnsubscribe {
    pub request_id: String,
    pub stream_id: String,
}

/// 事件串流已關閉，之後不再有這條串流的訊息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsUnsubscribeResult {
    pub request_id: String,
}

/// 列出主機上一個資料夾裡的子資料夾名稱(0.12.0 起,raychiutw/purrdr.host#26):給 HTTPS 主機的新增聊天瀏覽資料夾。只回資料夾名稱，絕不回檔案或檔案內容。唯讀，唯讀裝置也可用。主機服務解析 symlink 後再比對排除清單(x-excluded-file-locations)與主機服務資料夾:資料夾本身在排除清單就拒絕(directory_not_allowed),排除的子項目不列出。資料夾不存在或不是資料夾回 directory_not_found,讀不了回 directory_unreadable,path 不是絕對路徑或含 . 或 .. 段落回 invalid_message。~ 由 app 以 welcome 的 home_directory 換成路徑，主機服務不展開。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryList {
    pub request_id: String,
    /// 主機上的絕對路徑(不可含 . 或 .. 段落)。
    pub path: String,
}

/// 子資料夾名稱(0.12.0 起):含隱藏(點開頭)資料夾，不分大小寫排序;指向資料夾的 symlink 以解析後的目標判斷,目標在排除清單就不列。超過 1000 筆時只回前面的，truncated 為 true。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryListResult {
    pub request_id: String,
    /// 原樣回傳要求的 path(不解析 symlink)。
    pub path: String,
    /// 子資料夾的名稱(不含路徑)。
    pub directories: Vec<String>,
    /// 子資料夾多於上限，directories 只含排序後的前面一段。
    pub truncated: bool,
}

/// 主機取檔(raychiutw/purrdr.ios#227):查檔案狀態。只允許 herdr snapshot 目前回報的 Agent 工作目錄之下，或這條連線定位過的對話紀錄裡出現的路徑;主機服務解析 symlink 後再比對工作目錄與排除清單(x-excluded-file-locations)。唯讀，唯讀裝置也可用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStat {
    pub request_id: String,
    /// 主機上的絕對路徑(不可含 . 或 .. 段落)。
    pub path: String,
}

/// 檔案狀態。status 是 file 時帶版本(內容的 SHA-256)與大小;too_large 只帶大小。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStatResult {
    pub request_id: String,
    pub status: FileStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
}

/// file:可讀的一般檔案;missing:不存在;not_regular_file:資料夾、FIFO、裝置等;unreadable:沒有權限或讀取失敗;denied:存取規則不允許;too_large:超過 25 MiB。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileStatus {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "not_regular_file")]
    NotRegularFile,
    #[serde(rename = "unreadable")]
    Unreadable,
    #[serde(rename = "denied")]
    Denied,
    #[serde(rename = "too_large")]
    TooLarge,
}

/// 依版本讀 offset 起最多 length bytes(每次最多 1 MiB)。從頭讀(offset 0)或還沒有這個檔案的快照時，主機服務先檢查存取規則、把檔案讀成這條連線的快照並比對版本;之後的分塊從快照讀，分塊之間檔案被覆寫也不會拼出兩個版本。版本不符回 file_changed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRead {
    pub request_id: String,
    pub path: String,
    /// file.stat 回報的版本。
    pub version: String,
    pub offset: i64,
    pub length: i64,
}

/// status 是 file 時帶 offset 與 bytes(標準 base64,超過檔尾時為空);其他狀態同 file.stat,不帶內容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileReadResult {
    pub request_id: String,
    pub status: FileStatus,
    /// status 是 too_large 時的大小。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>,
}

/// 推播外掛代裝(raychiutw/purrdr.ios#261,0.6.0 起):查 herdr 是否已安裝推播外掛(purrdr.push)、這支裝置在這台主機是否已有推播配對(~/.purrdr/push-<device_id>/pairing)、其他已配對裝置數。唯讀，唯讀裝置也可用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushStatus {
    pub request_id: String,
    /// app 的推播裝置 id(與 SSH 路徑相同):1–64 個小寫英數與 -,不能是 plugin。不合格回 invalid_message。
    pub device_id: String,
}

/// 開啟這支裝置在這台主機的通知：寫入推播配對(與 SSH 路徑相同的 ~/.purrdr/push-<device_id>/pairing,已有就覆寫),外掛還沒安裝就以本機使用者身分執行 herdr plugin install raychiutw/purrdr.host/host-plugin --yes(來源固定，app 不能指定)。0.12.0 起主機服務能直接送推播(回覆的 direct_push 為 true)時只寫配對、不安裝外掛;主機服務不能接手(例如已裝舊版外掛 0.1.0)時行為不變。安裝失敗時還原成寫入前的樣子(原有的配對保留)並回 push_plugin_failed。重送一律代表「以這把 key 重新配對」(覆寫配對檔;外掛已裝就不重裝)。只改靜音請用 push.update_muted。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushInstall {
    pub request_id: String,
    /// app 的推播裝置 id(與 SSH 路徑相同):1–64 個小寫英數與 -,不能是 plugin。不合格回 invalid_message。
    pub device_id: String,
    pub pairing: PushPairing,
}

/// 關閉這支裝置在這台主機的通知：只移除這支裝置的配對資料夾;沒有其他裝置的配對、且外掛是 Purrdr 裝的(~/.purrdr/push-plugin/ 的紀錄)才執行 herdr plugin uninstall purrdr.push。主機服務直接送時沒有安裝紀錄，也就不會執行 uninstall。使用者自己裝的外掛不動。外掛移除失敗回 push_plugin_failed(配對已移除)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRemove {
    pub request_id: String,
    /// app 的推播裝置 id(與 SSH 路徑相同):1–64 個小寫英數與 -,不能是 plugin。不合格回 invalid_message。
    pub device_id: String,
}

/// 推播配對檔的內容(app 的 PushPairingFile):主機服務寫成 purrdr-push 1、endpoint=、key=、muted= 各一行。endpoint 必須是 https:// 開頭、不含空白與控制字元;key 是 64 位小寫 hex;muted 不能含控制字元。不合格回 invalid_message。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushPairing {
    /// 後台 /v1/push 的網址。
    pub endpoint: String,
    /// 推播配對密鑰(64 位小寫 hex);只寫進主機上的配對檔，不記進稽核紀錄。
    pub key: String,
    /// 這支裝置靜音的聊天(Agent 的 session 值)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<Vec<String>>,
}

/// 這台主機的推播狀態。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushStatusResult {
    pub request_id: String,
    /// herdr 已安裝 purrdr.push(不論是誰裝的)。
    pub plugin_installed: bool,
    /// 這支裝置在這台主機有推播配對。
    pub paired: bool,
    /// 其他已配對裝置數(不含這支;不提供其他裝置的識別資訊)。
    pub other_devices: i64,
    /// 0.12.0 起(選用;舊版主機服務沒有):主機服務會直接送 blocked／done 推播(Agent 觀察者執行中，且沒有裝會重複送的舊版外掛 purrdr.push 0.1.0)。true 時 push.install 不安裝外掛。沒有這個欄位等同 false。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_push: Option<bool>,
}

/// 開啟完成，帶完成後的推播狀態(欄位同 push.status.result)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushInstallResult {
    pub request_id: String,
    pub plugin_installed: bool,
    pub paired: bool,
    pub other_devices: i64,
    /// 0.12.0 起(選用):同 push.status.result。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_push: Option<bool>,
}

/// 關閉完成，帶完成後的推播狀態(欄位同 push.status.result)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushRemoveResult {
    pub request_id: String,
    pub plugin_installed: bool,
    pub paired: bool,
    pub other_devices: i64,
    /// 0.12.0 起(選用):同 push.status.result。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_push: Option<bool>,
}

/// 同步這支裝置在這台主機的靜音清單(app 不保存配對密鑰，不能重送 push.install):只以原子方式改寫配對檔的 muted 行，保留 key 與 endpoint,不呼叫 herdr。沒有配對檔回 push_not_paired;muted 含控制字元或空字串回 invalid_message。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushUpdateMuted {
    pub request_id: String,
    /// 同 push.install 的 device_id。
    pub device_id: String,
    /// 靜音的聊天(Agent 的 session 值),整份取代;空陣列表示取消全部靜音。
    pub muted: Vec<String>,
}

/// 靜音已更新。不問 herdr,plugin_installed 一律是 true(要確認外掛請用 push.status)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushUpdateMutedResult {
    pub request_id: String,
    pub plugin_installed: bool,
    pub paired: bool,
    pub other_devices: i64,
    /// 0.12.0 起(選用):不問 herdr,所以這個回覆不帶這個欄位(要確認請用 push.status)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_push: Option<bool>,
}

/// 送一則測試通知給這支裝置(raychiutw/purrdr.host#60,0.11.0 起):讀這支裝置的推播配對，對它的 endpoint 送 {key, event: test, host_name?}(後台 raychiutw/purrdr.server#19 組固定內容的通知，不受推播種類偏好影響、不去重)。只送給這支裝置，不經過主機外掛與 herdr。沒有配對、後台回 404 或 410 都回 push_not_paired(410 時刪掉這組配對，期間重新配對的不刪);連不上或後台回其他狀態碼回 push_send_failed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushTest {
    pub request_id: String,
    /// 同 push.install 的 device_id。
    pub device_id: String,
    /// app 主機清單上這台主機的名稱，轉給後台放進通知標題(「Test: <名稱> needs you」)。1–64 個字(同後台的上限)、不含控制字元，不合格回 invalid_message;沒帶時不送，後台用預設標題。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
}

/// 後台已接受測試通知(回 2xx)。實際送達由 APNs 決定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushTestResult {
    pub request_id: String,
}

/// Codex 的方案用量(raychiutw/purrdr.host#62,0.12.0 起):主機服務以本機使用者身分執行一次 codex app-server,讀 account/rateLimits/read(同 app 經 SSH 的 CodexPlanUsageHost),讀到就結束子程序。唯讀，唯讀裝置也可用。找不到 codex 回 codex_not_found;沒以 ChatGPT 登入回 codex_not_signed_in;其他失敗回 codex_failed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageCodex {
    pub request_id: String,
}

/// Codex 的方案用量。有多個 bucket 時用 Codex 本身的(codex)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageCodexResult {
    pub request_id: String,
    /// RFC 3339(UTC,到秒):主機服務讀取的時間。
    pub captured_at: String,
    /// 依 five_hour、week 排序;Codex 沒回的週期不出現(可能是空陣列)。
    pub periods: Vec<PlanUsagePeriod>,
}

/// Claude Code 的方案用量(raychiutw/purrdr.host#62,0.12.0 起):讀 Claude status line 包裝腳本存的 ~/.purrdr/claude-usage/usage.json(整台主機一份,輸入含 rate_limits 時才寫;安裝見 claude_usage.install)的 status_line_input.rate_limits.five_hour／seven_day。唯讀，唯讀裝置也可用。沒有檔案不是錯誤:回空的 periods;installed 告訴 app 是「還沒安裝」(false)還是「已安裝、Claude Code 還沒回報」(true 且沒有 captured_at)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageClaude {
    pub request_id: String,
}

/// Claude Code 的方案用量。已過 resets_at 的視窗不回(那個視窗已重置，檔案裡的比例過時了);缺少 used_percentage 或它為 null／不是數字的視窗略過;沒有 resets_at 的視窗照回(無法判斷是否過期，resets_at 欄位省略)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageClaudeResult {
    pub request_id: String,
    /// 主機上有 Claude 方案用量的安裝紀錄(~/.purrdr/claude-usage/journal.json;不論是 app 經 SSH 或主機服務裝的)。false 代表 app 該顯示「未安裝」。
    pub installed: bool,
    /// RFC 3339(UTC,到秒):包裝腳本存下這份用量的時間(usage.json 的 captured_at);還沒有 usage.json 時沒有這欄。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    /// 依 five_hour、week 排序(five_hour 對應 rate_limits.five_hour,week 對應 seven_day);可能是空陣列。
    pub periods: Vec<PlanUsagePeriod>,
}

/// 每個 Claude session 的 context 用量(raychiutw/purrdr.host#62、#64,0.12.0 起):讀包裝腳本每個 session 存的 ~/.purrdr/claude-usage/sessions/<session_id>.json 的 status_line_input.context_window。唯讀，唯讀裝置也可用。只讀這個資料夾裡的檔案;沒有檔案的 session 不出現在結果。session_id 與腳本同一規則(1–128 個 A-Za-z0-9._- 字元,不是 . 或 ..),不合格回 invalid_message。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSessionContext {
    pub request_id: String,
    /// 要讀的 session(herdr 回報的 agent session)。省略時回資料夾裡全部(依 captured_at 新到舊,最多 200 筆)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ids: Option<Vec<String>>,
}

/// 各 session 的 context 用量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSessionContextResult {
    pub request_id: String,
    /// 有 session 檔且讀得懂的 session;指定 session_ids 時依請求順序,省略時依 captured_at 新到舊。
    pub sessions: Vec<SessionContext>,
}

/// 一個 Claude session 最近一次 status line 回報的 context 用量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionContext {
    pub session_id: String,
    /// RFC 3339(UTC,到秒):包裝腳本存下這份回報的時間。
    pub captured_at: String,
    /// context_window.used_percentage(夾在 0～100);Claude Code 沒回或為 null(第一次呼叫 API 前)時沒有這欄。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_percentage: Option<f64>,
    /// context_window.context_window_size(token 數);沒有時沒有這欄。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_size: Option<i64>,
}

/// Claude 方案用量包裝的安裝狀態(raychiutw/purrdr.host#62,0.12.0 起)。唯讀，唯讀裝置也可用。安裝紀錄壞了回 host_write_failed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsageStatus {
    pub request_id: String,
}

/// Claude 方案用量包裝的狀態(claude_usage.install／remove 的結果欄位相同)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsageStatusResult {
    pub request_id: String,
    /// 有安裝紀錄(~/.purrdr/claude-usage/journal.json)。
    pub installed: bool,
    /// settings.json(~/.claude,有 CLAUDE_CONFIG_DIR 時是該資料夾)的 statusLine.command 現在是 Purrdr 的包裝。使用者安裝後自己換掉 status line 時是 false。
    pub wrapper_active: bool,
    /// Claude Code 的設定資料夾存在(不存在時 claude_usage.install 回 claude_not_found)。
    pub claude_found: bool,
}

/// 安裝 Claude status line 包裝(raychiutw/purrdr.host#62,0.12.0 起;需要使用者同意,app 先取得同意再送)。與 app 經 SSH 的 ClaudePlanUsageHost／ReversibleHostWrite 相同:把 claude-usage/statusline.sh(Windows 主機是 statusline.ps1,指令為 powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '<家目錄>/.purrdr/claude-usage/statusline.ps1')寫到 ~/.purrdr/claude-usage/,使用者原本的 statusLine.command 存進 original-command(包裝本身永遠不會被存成原本的),把 settings.json(~/.claude,有 CLAUDE_CONFIG_DIR 時是該資料夾)的 statusLine 換成包裝,其餘設定保留。安裝前的內容記在 journal.json(格式同 app 的 HostWriteJournal),重複安裝沿用第一次的紀錄;只在讀取之後設定檔沒被改過才寫入(否則回 host_write_failed)。設定檔不是合法 JSON 物件回 claude_settings_not_json、不動檔案;Claude Code 的設定資料夾不存在回 claude_not_found。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsageInstall {
    pub request_id: String,
}

/// 安裝完成,帶安裝後的狀態(欄位同 claude_usage.status.result)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsageInstallResult {
    pub request_id: String,
    pub installed: bool,
    pub wrapper_active: bool,
    pub claude_found: bool,
}

/// 還原 Claude status line 包裝(raychiutw/purrdr.host#62,0.12.0 起;app 以 SSH 或主機服務裝的都能還原)。設定檔仍是安裝寫入的內容 → 換回安裝前的 bytes(原本不存在就刪除);安裝後使用者又改過 → 只撤掉 statusLine 的修改(statusLine 已被使用者換掉就不動),其餘保留;最後移除 ~/.purrdr/claude-usage/。沒有安裝紀錄時什麼都不做。目前的設定檔不是合法 JSON 物件且需要合併時回 claude_settings_not_json、不動檔案。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsageRemove {
    pub request_id: String,
}

/// 還原完成,帶還原後的狀態(欄位同 claude_usage.status.result)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsageRemoveResult {
    pub request_id: String,
    pub installed: bool,
    pub wrapper_active: bool,
    pub claude_found: bool,
}

/// 方案用量的一個週期。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanUsagePeriod {
    pub kind: PlanUsagePeriodKind,
    /// 已用比例(夾在 0～100,不四捨五入：app 依它算剩餘比例與進度條)。
    pub used_percentage: f64,
    /// RFC 3339(UTC,到秒):這個週期重置的時間;Codex 沒回、或 Claude 的 status line 輸入沒帶 resets_at(缺少或為 null)時沒有這欄。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

/// 週期種類：週期長度不到一天是 five_hour,否則是 week(Codex 沒回長度時，primary 是 five_hour、secondary 是 week)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlanUsagePeriodKind {
    #[serde(rename = "five_hour")]
    FiveHour,
    #[serde(rename = "week")]
    Week,
}

/// 上傳圖片或檔案(raychiutw/purrdr.host#16):開始一次分塊上傳。主機服務在系統暫存目錄底下的上傳資料夾(x-upload-folder)建暫存檔，檔名由主機服務產生(不採用 app 給的名稱),超過 7 天的上傳檔定期刪除。size 超過 x-maximum-file-size 回 upload_too_large。中繼資料剝除在 app 端做，主機服務不解析內容。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileUploadStart {
    /// 也是這次上傳的 upload_id。
    pub request_id: String,
    /// 總 bytes。0 時仍要送一塊 offset 0、內容為空的 chunk,收到後才存檔並回傳路徑。
    pub size: i64,
    /// 主機檔名的副檔名(不含點):1–16 個英數字元，存成小寫;沒帶時不加副檔名。不合格回 invalid_message。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
}

/// 上傳已開始，之後以 upload_id 送分塊。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileUploadStartResult {
    pub request_id: String,
    pub upload_id: String,
}

/// 依序送一塊內容(解碼後最多 x-maximum-file-chunk bytes,超過回 upload_too_large)。offset 必須等於已收到的 bytes,超過 size 或 upload_id 不是這條連線開始的都回 invalid_message,失敗後這次上傳作廢。收滿 size 時存成正式檔。連線結束時未完成的上傳刪掉。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileUploadChunk {
    pub request_id: String,
    pub upload_id: String,
    pub offset: i64,
    /// 標準 base64。
    pub bytes: String,
}

/// 分塊已寫入。收滿 size 時帶 path:主機上的絕對路徑，app 放進 prompt。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileUploadChunkResult {
    pub request_id: String,
    pub upload_id: String,
    /// 目前已收到的 bytes。
    pub received: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// 列出這台主機已配對的裝置(raychiutw/purrdr.host#19;不含已撤銷的)。唯讀，唯讀裝置也可用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicesList {
    pub request_id: String,
}

/// 已配對的裝置，依配對先後。不含權杖或其雜湊。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicesListResult {
    pub request_id: String,
    pub devices: Vec<PairedDevice>,
}

/// 一支已配對的裝置。時間是 RFC 3339 UTC(到秒)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: String,
    /// 配對時 app 給的裝置名稱。
    pub name: String,
    pub read_only: bool,
    pub paired_at: String,
    /// 是發出這個請求的裝置。
    pub current: bool,
}

/// 撤銷一支已配對的裝置：之後它的每個 WSS 訊息都以 device_token_revoked 與 close code 4401 拒絕。已撤銷的再撤銷一次也成功;不認得的 device_id 回 device_unknown;不能撤銷發出請求的這支裝置(回 invalid_message,撤銷自己用 POST /v1/revoke)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicesRevoke {
    pub request_id: String,
    /// devices.list.result 裡的 device_id。
    pub device_id: String,
}

/// 已撤銷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicesRevokeResult {
    pub request_id: String,
    pub device_id: String,
}

/// 從 app 管理主機服務(raychiutw/purrdr.host#18):查已安裝版本、最新版本(問 Release 來源)、自動更新開關、上次檢查或更新的結果與退回過的版本。唯讀，唯讀裝置也可用。沒有安裝(開發中直接執行)時回 host_service_not_installed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostServiceStatus {
    pub request_id: String,
}

/// 主機服務的更新狀態(同主機上的 update --check 與 version)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostServiceStatusResult {
    pub request_id: String,
    /// 安裝好的執行檔回報的版本;更新後重啟前可能比 welcome 的 host_service_version 新。
    pub installed_version: String,
    /// Release 來源的最新版本;取不到時沒有這欄，改帶 latest_error。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    /// 取不到最新版本的原因(給開發者看)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_error: Option<String>,
    /// 最新版本比已安裝的大(只往更大的版本更新)。
    pub update_available: bool,
    /// 安裝紀錄的自動更新開關。
    pub auto_update: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<HostServiceUpdateRecord>,
    /// 上次更新後起不來、已換回舊版的版本;背景自動更新不再重試它，host_service.update 仍會再試。更新成功後清掉。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back: Option<String>,
}

/// 上一次檢查或更新的時間與結果(update.json)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostServiceUpdateRecord {
    /// RFC 3339(UTC,到秒)。
    pub checked_at: String,
    /// 主機上 version 顯示的同一句話，例如 updated 0.5.0 -> 0.5.1、update failed: …(給人看，不解析)。
    pub result: String,
}

/// 立即更新：有更大的版本就以獨立程序執行安裝好的 update --yes(與主機上手動更新同一條流程：update.lock、核對 SHA256SUMS、起不來就換回),回應在重啟前送出。主機服務接著重啟、連線中斷，app 重新連線後以 welcome 的 host_service_version 確認結果，失敗退回時 host_service.status 看得到 rolled_back 與 last_update。已有更新在進行回 update_in_progress;取不到最新版本或啟動不了更新程序回 update_failed。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostServiceUpdate {
    pub request_id: String,
}

/// started:更新程序已啟動，from_version → to_version;up_to_date:已是最新，沒有更新(不重啟)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostServiceUpdateResult {
    pub request_id: String,
    pub outcome: HostServiceUpdateOutcome,
    /// 已安裝的版本。
    pub from_version: String,
    /// 最新版本(started 時是要更新到的版本)。
    pub to_version: String,
}

/// 立即更新的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostServiceUpdateOutcome {
    #[serde(rename = "started")]
    Started,
    #[serde(rename = "up_to_date")]
    UpToDate,
}

/// 開關背景自動更新(同主機上的 auto-update on|off,改安裝紀錄的 auto_update);背景服務下一次檢查時就照新的設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostServiceSetAutoUpdate {
    pub request_id: String,
    pub enabled: bool,
}

/// 已改好，帶改完後的開關。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostServiceSetAutoUpdateResult {
    pub request_id: String,
    pub auto_update: bool,
}

/// POST /v1/pair 的本文：以配對碼換裝置權杖。QR code 的內容是 purrdr://pair?address=<HTTPS 位址>&code=<配對碼>。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairRequest {
    /// 大小寫與連字號不拘，例如 K7QM-2XPA。
    pub pairing_code: String,
    /// 顯示在主機裝置列表的名稱。
    pub device_name: String,
}

/// 配對成功。device_token 只在這裡出現一次，主機只存雜湊。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: String,
    pub device_token: String,
    /// 唯讀裝置的寫入類操作會被拒絕(read_only_device)。
    pub read_only: bool,
}

/// 請求失敗(WSS 訊息，也是配對與撤銷 HTTPS 請求失敗時的本文)。訊息本身無法解析時沒有 request_id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub code: ErrorCode,
    /// 給開發者看的說明，不顯示給使用者。
    pub message: String,
    /// code 是 herdr_error 時，herdr 原本的錯誤碼。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr_code: Option<String>,
    /// code 是 protocol_version_unsupported 時，主機服務支援的協定版本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_protocol_versions: Option<Vec<i64>>,
}

/// 錯誤種類。WSS 收到 device_token_invalid／device_token_revoked 後，主機服務以 close code 4401 關閉連線;HTTPS 請求回 401。transcript_not_allowed:session 不是 herdr 目前回報的，或路徑不是這條連線定位過的(或其 session 資料夾 subagents/ 底下的 .jsonl);transcript_unsupported:這種 agent 的對話紀錄主機服務還讀不了;file_changed:file.read 時檔案已不是要求的版本;push_plugin_failed:herdr plugin list／install／uninstall 失敗(message 是 herdr 的輸出);push_not_paired:push.update_muted／push.test 時這支裝置在這台主機沒有推播配對(push.test 另含後台回 404／410);codex_not_found:usage.codex 找不到 codex 執行檔;codex_not_signed_in:Codex 沒以 ChatGPT 登入(或用 API key);codex_failed:codex app-server 回報失敗、沒有回應或回應看不懂(message 是原因);claude_not_found:claude_usage.install 時 Claude Code 的設定資料夾(~/.claude 或 CLAUDE_CONFIG_DIR)不存在;claude_settings_not_json:Claude 的 settings.json 不是合法的 JSON 物件，沒有改動檔案(0.12.0 起);host_write_failed:claude_usage.* 的檔案無法安全修改(安裝紀錄壞了、設定檔不是一般檔案或讀不到、讀取之後被改過;message 是原因);push_send_failed:push.test 連不上後台或後台回其他狀態碼(message 是後台的錯誤碼，例如 subscription_required、rate_limited、apns_failed;沒有錯誤碼時是 http <狀態碼> 或連線錯誤);device_unknown:devices.revoke 的 device_id 不是這台主機配對過的裝置;upload_too_large:上傳的總大小或分塊超過 x-maximum-file-size／x-maximum-file-chunk;host_service_not_installed:主機服務不是安裝好的那份，不能從 app 管理;update_in_progress:已有另一個更新在進行;update_failed:取不到最新版本、改不了設定或啟動不了更新程序(message 是原因);pane_changed:送出前比對 expected_session／expected_screen 不符，沒有送出;cwd_not_allowed:git.status／git.diff 的 cwd 不是 herdr 目前回報的 Agent 工作目錄;not_git_repository:cwd 不在 git repo 的工作目錄裡;queue_full:這個 pane 的 Send When Done 佇列已有 20 則;queue_item_unknown:queue.remove／queue.reorder 的 item_id 不在佇列裡;其他 transcript_* 對應行為規格 §4.7 的缺席、不可讀、有歧義、已壓縮。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "invalid_message")]
    InvalidMessage,
    #[serde(rename = "handshake_required")]
    HandshakeRequired,
    #[serde(rename = "protocol_version_unsupported")]
    ProtocolVersionUnsupported,
    #[serde(rename = "herdr_unavailable")]
    HerdrUnavailable,
    #[serde(rename = "herdr_capability_missing")]
    HerdrCapabilityMissing,
    #[serde(rename = "herdr_error")]
    HerdrError,
    #[serde(rename = "device_token_invalid")]
    DeviceTokenInvalid,
    #[serde(rename = "device_token_revoked")]
    DeviceTokenRevoked,
    #[serde(rename = "read_only_device")]
    ReadOnlyDevice,
    #[serde(rename = "pairing_code_invalid")]
    PairingCodeInvalid,
    #[serde(rename = "pairing_code_expired")]
    PairingCodeExpired,
    #[serde(rename = "transcript_not_allowed")]
    TranscriptNotAllowed,
    #[serde(rename = "transcript_unsupported")]
    TranscriptUnsupported,
    #[serde(rename = "transcript_absent")]
    TranscriptAbsent,
    #[serde(rename = "transcript_unreadable")]
    TranscriptUnreadable,
    #[serde(rename = "transcript_ambiguous")]
    TranscriptAmbiguous,
    #[serde(rename = "transcript_compressed")]
    TranscriptCompressed,
    #[serde(rename = "file_changed")]
    FileChanged,
    #[serde(rename = "push_plugin_failed")]
    PushPluginFailed,
    #[serde(rename = "push_not_paired")]
    PushNotPaired,
    #[serde(rename = "push_send_failed")]
    PushSendFailed,
    #[serde(rename = "codex_not_found")]
    CodexNotFound,
    #[serde(rename = "codex_not_signed_in")]
    CodexNotSignedIn,
    #[serde(rename = "codex_failed")]
    CodexFailed,
    #[serde(rename = "claude_not_found")]
    ClaudeNotFound,
    #[serde(rename = "claude_settings_not_json")]
    ClaudeSettingsNotJson,
    #[serde(rename = "host_write_failed")]
    HostWriteFailed,
    #[serde(rename = "upload_too_large")]
    UploadTooLarge,
    #[serde(rename = "device_unknown")]
    DeviceUnknown,
    #[serde(rename = "host_service_not_installed")]
    HostServiceNotInstalled,
    #[serde(rename = "update_in_progress")]
    UpdateInProgress,
    #[serde(rename = "update_failed")]
    UpdateFailed,
    #[serde(rename = "pane_changed")]
    PaneChanged,
    #[serde(rename = "cwd_not_allowed")]
    CwdNotAllowed,
    #[serde(rename = "not_git_repository")]
    NotGitRepository,
    #[serde(rename = "directory_not_found")]
    DirectoryNotFound,
    #[serde(rename = "directory_not_allowed")]
    DirectoryNotAllowed,
    #[serde(rename = "directory_unreadable")]
    DirectoryUnreadable,
    #[serde(rename = "queue_full")]
    QueueFull,
    #[serde(rename = "queue_item_unknown")]
    QueueItemUnknown,
    #[serde(rename = "internal_error")]
    InternalError,
}
