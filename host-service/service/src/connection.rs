//! 一條 app 的 WebSocket 連線：解析協定訊息，轉成 herdr 請求。只處理協定定義的訊息種類。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::agent_observer::{AgentObserver, MAXIMUM_PROMPT_BYTES, MAXIMUM_QUEUED_PROMPTS, QueueFailure};
use crate::audit_log::{AuditEvent, AuditLog, RevokedBy};
use crate::claude_usage::ClaudeUsage;
use crate::codex_usage::{self, CodexCli, CodexUsageFailure};
use crate::device_store::{Device, DeviceStore, RevokeFailure};
use crate::directory_list::{self, ListError};
use crate::git_changes::{self, GitFailure};
use crate::herdr_cli::HerdrCli;
use crate::herdr_client::{HerdrClient, HerdrFailure, HerdrSubscription};
use crate::host_files::{self, FileAccessCache, FileAccessEnvironment, FileAccessRules, FileSnapshot, Located, Purpose, RequestPacing};
use crate::http::{ServiceContext, blocking};
use crate::pane_guard::{self, GuardFailure};
use crate::pane_screen;
use crate::pane_terminal::{self, PaneTerminal};
use crate::protocol::{
    Agent, AgentPrompt, AgentPromptResult, AgentSession, AgentStatus, AgentsListResult, ClientMessage, DevicesListResult, DevicesRevokeResult, DirectoryList,
    DirectoryListResult, ErrorCode, ErrorMessage, EventsEnded, EventsSubscribe, EventsSubscribeResult, EventsUnsubscribeResult, FileRead, FileReadResult,
    FileStat, FileStatResult, FileStatus, FileUploadChunk, FileUploadChunkResult, FileUploadStart, FileUploadStartResult, GitDiff, GitDiffResult, GitStatus,
    GitStatusResult, HerdrServer, PROTOCOL_VERSION, PairedDevice, PaneSendKeys, PaneSendKeysResult, PaneSendText, PaneSendTextResult, PaneTerminalOpen,
    PaneTerminalOpenResult, PushTestResult, QueueAdd, QueueAddResult, QueueList, QueueListResult, QueueRemove, QueueRemoveResult, QueueReorder,
    QueueReorderResult, ServerMessage, SessionsList, SessionsListResult, SubscribedEvent, Tab, TerminalScrollDirection, TerminalScrollSource, TranscriptLocate,
    UsageCodexResult, Welcome, Workspace,
};
use crate::protocol::{Layout, LayoutPane, LayoutRect, LayoutSplit, Pane};
use crate::push_plugin::{PushFailure, PushPlugin, PushState};
use crate::remote_management::{self, ServiceManagement};
use crate::session_history;
use crate::structure_operations;
use crate::text_format::rfc3339;
use crate::transcript::{AgentSessionInfo, TranscriptEnvironment};
use crate::transcript_session::TranscriptSession;
use crate::uploads::{ChunkOutcome, PendingUpload, UploadDirectory, UploadFailure};

/// 權杖無效或已撤銷時的 close code(4000–4999 由應用程式自訂)。
pub const UNAUTHORIZED_CLOSE_CODE: u16 = 4401;

/// 主機服務內部錯誤的 close code(RFC 6455)。
const INTERNAL_ERROR_CLOSE_CODE: u16 = 1011;

/// 送出確認:文字寫進終端機後等 Agent 反應的時間，與補 Enter 之後再等的時間(與 app 的 SSH 作法相同)。
const CONFIRM_WAIT: Duration = Duration::from_millis(3500);
const ENTER_WAIT: Duration = Duration::from_millis(2500);

/// 送出 close 後等 app 回 close 的上限。
const CLOSE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// 驗證權杖後處理到連線關閉為止。每個訊息都重新驗證，撤銷立即生效。
pub async fn serve(socket: WebSocket, context: ServiceContext, device_token: Option<String>) {
    let device = match context.authenticate(device_token.as_deref()).await {
        Ok(device) => device,
        Err(code) => return reject(socket, code).await,
    };
    let devices = context.devices.clone();
    let device_id = device.device_id.clone();
    if let Err(error) = blocking(move || devices.record_connection(&device_id, SystemTime::now())).await {
        context.log.record(&format!("cannot record connection: {error}"));
    }
    context.audit.record(
        AuditEvent::Connected {
            device_id: device.device_id.clone(),
        },
        SystemTime::now(),
    );

    // 讀取、處理、送出分開：處理請求(等 herdr)時仍持續收訊息、回 pong,事件與心跳也能隨時送出。
    let heartbeat = context.heartbeat;
    let (sink, mut incoming) = socket.split();
    let (outbound, outbound_queue) = mpsc::unbounded_channel();
    // 串流(對話紀錄追蹤)推送的訊息也由送出段送出;連線結束時 `connection` 一起 drop,串流隨之停止。
    let (pushes, pushed) = mpsc::unbounded_channel();
    // Pane 終端機的畫面更新：有界，送出段跟不上時 herdr 那端等(herdr 只送最新畫面)。
    let (terminal_frames, terminal_frame_queue) = mpsc::channel(TERMINAL_FRAME_CAPACITY);
    let writer = tokio::spawn(write_outbound(sink, outbound_queue, pushed, terminal_frame_queue, heartbeat.interval));
    let (requests, request_queue) = mpsc::unbounded_channel();
    let connection = ClientConnection {
        herdr: context.herdr.clone(),
        herdr_cli: context.herdr_cli.clone(),
        codex_cli: context.codex_cli.clone(),
        audit: context.audit.clone(),
        outbound: outbound.clone(),
        handshaken: false,
        transcripts: TranscriptSession::new(context.transcripts.clone(), pushes),
        transcript_environment: context.transcripts.clone(),
        subscriptions: HashMap::new(),
        accepted_subscription: None,
        terminals: HashMap::new(),
        terminal_frames,
        files: context.files.clone(),
        file_cache: Arc::new(std::sync::Mutex::new(FileAccessCache::default())),
        file_pacing: RequestPacing::default(),
        file_snapshot: None,
        push: context.push.clone(),
        uploads: context.uploads.clone(),
        pending_uploads: HashMap::new(),
        service_management: context.service_management.clone(),
        devices: context.devices.clone(),
        observer: context.observer.clone(),
        queue_changes: context
            .observer
            .as_ref()
            .map(|observer| forward_queue_changes(observer.subscribe(), outbound.clone())),
    };
    let handler = tokio::spawn(handle_requests(connection, request_queue, context, device_token));
    // 逾時沒收到任何 frame(含 pong)就當作 app 已經不在。
    while let Ok(Some(Ok(message))) = tokio::time::timeout(heartbeat.timeout, incoming.next()).await {
        match message {
            Message::Text(text) => {
                let _ = requests.send(text.to_string());
            }
            Message::Binary(_) => {
                let _ = outbound.send(Outbound::Message(error(None, ErrorCode::InvalidMessage, "binary messages are not supported")));
            }
            Message::Close(_) => break,
            _ => continue,
        }
    }
    // 結束處理就放掉這條連線的 herdr 訂閱，Pane 終端機的子行程一併收尾。
    handler.abort();
    writer.abort();
}

/// Pane 終端機畫面更新等待送出的上限。
const TERMINAL_FRAME_CAPACITY: usize = 32;

/// 要送給 app 的訊息。
enum Outbound {
    Message(ServerMessage),
    /// 送出 close frame 後不再送任何東西。
    Close(CloseFrame),
}

/// 依序送出訊息，並每 `ping_interval` 送一次 ping。
async fn write_outbound(
    mut sink: SplitSink<WebSocket, Message>,
    mut queue: mpsc::UnboundedReceiver<Outbound>,
    mut pushes: mpsc::UnboundedReceiver<ServerMessage>,
    mut terminal_frames: mpsc::Receiver<ServerMessage>,
    ping_interval: Duration,
) {
    let mut pings = tokio::time::interval_at(tokio::time::Instant::now() + ping_interval, ping_interval);
    loop {
        let (frame, last) = tokio::select! {
            item = queue.recv() => match item {
                Some(Outbound::Message(message)) => (Message::Text(serde_json::to_string(&message).expect("協定訊息可序列化").into()), false),
                Some(Outbound::Close(frame)) => (Message::Close(Some(frame)), true),
                None => return,
            },
            Some(push) = pushes.recv() => (Message::Text(serde_json::to_string(&push).expect("協定訊息可序列化").into()), false),
            Some(frame) = terminal_frames.recv() => (Message::Text(serde_json::to_string(&frame).expect("協定訊息可序列化").into()), false),
            _ = pings.tick() => (Message::Ping(Default::default()), false),
        };
        if sink.send(frame).await.is_err() || last {
            return;
        }
    }
}

/// 依序處理請求;每個訊息都重新驗證權杖，被拒時送錯誤與 close frame 後停止。
async fn handle_requests(mut connection: ClientConnection, mut queue: mpsc::UnboundedReceiver<String>, context: ServiceContext, device_token: Option<String>) {
    while let Some(text) = queue.recv().await {
        match context.authenticate(device_token.as_deref()).await {
            Ok(device) => connection.handle(&text, &device).await,
            Err(code) => {
                let (message, frame) = rejection(code);
                let _ = connection.outbound.send(Outbound::Message(message));
                let _ = connection.outbound.send(Outbound::Close(frame));
                return;
            }
        }
    }
}

/// 要先送 hello 的訊息;不必的是 hello 本身、推播代裝，以及作用在 hello 後才開的串流、上傳與對話紀錄上的訊息。
fn requires_handshake(message: &ClientMessage) -> bool {
    !matches!(
        message,
        ClientMessage::Hello(_)
            | ClientMessage::TranscriptWindow(_)
            | ClientMessage::TranscriptFollow(_)
            | ClientMessage::TranscriptUnfollow(_)
            | ClientMessage::TranscriptPreviews(_)
            | ClientMessage::TranscriptSubagent(_)
            | ClientMessage::TranscriptWorkflow(_)
            | ClientMessage::TranscriptDelegation(_)
            | ClientMessage::EventsUnsubscribe(_)
            | ClientMessage::PaneTerminalInput(_)
            | ClientMessage::PaneTerminalResize(_)
            | ClientMessage::PaneTerminalScroll(_)
            | ClientMessage::PaneTerminalClose(_)
            | ClientMessage::FileUploadChunk(_)
            | ClientMessage::PushStatus(_)
            | ClientMessage::PushInstall(_)
            | ClientMessage::PushUpdateMuted(_)
            | ClientMessage::PushRemove(_)
            | ClientMessage::PushTest(_)
    )
}

/// 權杖被拒時的錯誤訊息與 close frame。
fn rejection(code: ErrorCode) -> (ServerMessage, CloseFrame) {
    let (close_code, reason, message) = match code {
        ErrorCode::DeviceTokenRevoked => (UNAUTHORIZED_CLOSE_CODE, "device_token_revoked", "device token was revoked"),
        ErrorCode::DeviceTokenInvalid => (UNAUTHORIZED_CLOSE_CODE, "device_token_invalid", "missing or unknown device token"),
        _ => (INTERNAL_ERROR_CLOSE_CODE, "internal_error", "host service cannot read its device store"),
    };
    (
        error(None, code, message),
        CloseFrame {
            code: close_code,
            reason: reason.into(),
        },
    )
}

/// 連線一開始權杖就被拒：送出錯誤訊息後以 close code 關閉。
async fn reject(mut socket: WebSocket, code: ErrorCode) {
    let (message, frame) = rejection(code);
    let _ = send(&mut socket, &message).await;
    let _ = socket.send(Message::Close(Some(frame))).await;
    // 等 app 回 close 再斷線;直接斷線會讓 app 收到 TCP reset 而讀不到 close code。
    let _ = tokio::time::timeout(CLOSE_HANDSHAKE_TIMEOUT, async {
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}

async fn send(socket: &mut WebSocket, message: &ServerMessage) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).expect("協定訊息可序列化");
    socket.send(Message::Text(text.into())).await
}

/// 一條 app WebSocket 連線的狀態(不是 herdr 的 session)。drop 時結束這條連線的 herdr 訂閱。
struct ClientConnection {
    herdr: HerdrClient,
    audit: AuditLog,
    outbound: mpsc::UnboundedSender<Outbound>,
    handshaken: bool,
    transcripts: TranscriptSession,
    /// 續接 session 時找對話紀錄的位置。
    transcript_environment: TranscriptEnvironment,
    /// 事件串流識別碼 → 轉送 herdr 事件的 task。
    subscriptions: HashMap<String, JoinHandle<()>>,
    /// herdr 剛接受、等訂閱結果送出後才開始轉送的訂閱。
    accepted_subscription: Option<(String, HerdrSubscription)>,
    herdr_cli: HerdrCli,
    codex_cli: CodexCli,
    /// 開著的 Pane 終端機，以串流識別碼(開啟請求的 request_id)為 key;drop 時子行程收尾。
    terminals: HashMap<String, PaneTerminal>,
    terminal_frames: mpsc::Sender<ServerMessage>,
    files: FileAccessEnvironment,
    /// 主機取檔：對話紀錄裡 Agent 寫出的路徑與版本快取(在 blocking 執行緒用)。
    file_cache: Arc<std::sync::Mutex<FileAccessCache>>,
    file_pacing: RequestPacing,
    /// 主機取檔最近一次從頭讀的快照。
    file_snapshot: Option<FileSnapshot>,
    push: PushPlugin,
    /// 家目錄的上傳資料夾;找不到家目錄時不能上傳。
    uploads: Option<UploadDirectory>,
    /// 進行中的上傳，以 upload_id(開始請求的 request_id)為 key;連線結束時一併刪掉暫存檔。
    pending_uploads: HashMap<String, PendingUpload>,
    service_management: Option<ServiceManagement>,
    devices: DeviceStore,
    /// Send When Done 佇列;觀察者沒啟動時 None。
    observer: Option<AgentObserver>,
    /// 把 queue.changed 轉給這條連線的 task。
    queue_changes: Option<JoinHandle<()>>,
}

impl Drop for ClientConnection {
    fn drop(&mut self) {
        for forwarding in self.subscriptions.values() {
            forwarding.abort();
        }
        if let Some(queue_changes) = &self.queue_changes {
            queue_changes.abort();
        }
    }
}

impl ClientConnection {
    /// Claude 方案用量:位置只由家目錄與 `CLAUDE_CONFIG_DIR` 決定，app 不能指定路徑。
    fn claude_usage(&self) -> ClaudeUsage {
        ClaudeUsage::new(self.transcript_environment.home.clone())
    }

    async fn handle(&mut self, text: &str, device: &Device) {
        // Pane 終端機的輸入與關閉沒有回應。
        if let Some(reply) = self.reply(text, device).await {
            let _ = self.outbound.send(Outbound::Message(reply));
        }
        // 訂閱結果送出後才開始轉送，事件不會比結果早到。
        if let Some((stream_id, subscription)) = self.accepted_subscription.take() {
            self.start_forwarding(stream_id, subscription);
        }
    }

    async fn reply(&mut self, text: &str, device: &Device) -> Option<ServerMessage> {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Some(error(None, ErrorCode::InvalidMessage, "message is not valid JSON"));
        };
        let request_id = value.get("request_id").and_then(Value::as_str).map(str::to_owned);
        let message = match serde_json::from_value::<ClientMessage>(value.clone()) {
            Ok(message) => message,
            Err(decoding_error) => return Some(error(request_id, ErrorCode::InvalidMessage, &decoding_error.to_string())),
        };
        if message.is_write() {
            // 只記操作種類與目標;串流輸入不記，允許的上傳分塊改在完成或失敗時記一筆 upload。
            let allowed_upload_chunk = matches!(message, ClientMessage::FileUploadChunk(_)) && !device.read_only;
            if request_id.is_some() && !allowed_upload_chunk {
                self.audit.record(
                    AuditEvent::Write {
                        device_id: device.device_id.clone(),
                        operation: value["type"].as_str().unwrap_or_default().to_owned(),
                        workspace_id: value["workspace_id"].as_str().map(str::to_owned),
                        pane_id: value["pane_id"].as_str().map(str::to_owned),
                        allowed: !device.read_only,
                    },
                    SystemTime::now(),
                );
            }
            if device.read_only {
                return Some(error(request_id, ErrorCode::ReadOnlyDevice, "this device is paired read-only"));
            }
        }
        if requires_handshake(&message) && !self.handshaken {
            return Some(error(request_id, ErrorCode::HandshakeRequired, "send hello first"));
        }
        let reply = match message {
            ClientMessage::Hello(hello) => self.hello(hello.request_id, hello.protocol_version, device.read_only).await,
            ClientMessage::AgentsList(request) => self.agents_list(request.request_id).await,
            ClientMessage::PaneSendKeys(request) => self.pane_send_keys(request).await,
            ClientMessage::TranscriptLocate(request) => self.transcript_locate(request).await,
            ClientMessage::TranscriptWindow(request) => self.transcripts.window(request).await,
            ClientMessage::TranscriptFollow(request) => self.transcripts.follow(request).await,
            ClientMessage::TranscriptUnfollow(request) => self.transcripts.unfollow(request),
            ClientMessage::TranscriptPreviews(request) => self.transcripts.previews(request).await,
            ClientMessage::TranscriptSubagent(request) => self.transcripts.subagent(request).await,
            ClientMessage::TranscriptWorkflow(request) => self.transcripts.workflow(request).await,
            ClientMessage::TranscriptDelegation(request) => self.transcripts.delegation(request).await,
            ClientMessage::PaneSendText(request) => self.pane_send_text(request).await,
            ClientMessage::PaneRead(request) => pane_screen::read(&self.herdr, request).await,
            ClientMessage::SessionsList(request) => self.sessions_list(request).await,
            ClientMessage::GitStatus(request) => self.git_status(request).await,
            ClientMessage::GitDiff(request) => self.git_diff(request).await,
            ClientMessage::QueueList(request) => self.queue_list(request),
            ClientMessage::QueueAdd(request) => self.queue_add(request),
            ClientMessage::QueueRemove(request) => self.queue_remove(request),
            ClientMessage::QueueReorder(request) => self.queue_reorder(request),
            ClientMessage::AgentPrompt(request) => self.agent_prompt(request).await,
            ClientMessage::EventsSubscribe(request) => self.events_subscribe(request).await,
            ClientMessage::EventsUnsubscribe(request) => {
                if let Some(forwarding) = self.subscriptions.remove(&request.stream_id) {
                    forwarding.abort();
                }
                ServerMessage::EventsUnsubscribeResult(EventsUnsubscribeResult {
                    request_id: request.request_id,
                })
            }
            ClientMessage::PaneTerminalOpen(request) => self.open_terminal(request, device),
            ClientMessage::PaneTerminalInput(input) => {
                if input.bytes.len() > pane_terminal::MAX_INPUT_BASE64_LENGTH {
                    return Some(error(None, ErrorCode::InvalidMessage, "terminal input is larger than 64 KiB"));
                }
                self.terminal(&input.stream_id)?.input(input.bytes).await;
                return None;
            }
            ClientMessage::PaneTerminalResize(resize) => {
                if !pane_terminal::is_valid_size(resize.columns, resize.rows) {
                    return Some(error(None, ErrorCode::InvalidMessage, "terminal size is out of range"));
                }
                self.terminal(&resize.stream_id)?.resize(resize.columns, resize.rows).await;
                return None;
            }
            ClientMessage::PaneTerminalScroll(scroll) => {
                let direction = match scroll.direction {
                    TerminalScrollDirection::Up => "up",
                    TerminalScrollDirection::Down => "down",
                };
                let source = match scroll.source {
                    Some(TerminalScrollSource::PageKey) => "page_key",
                    Some(TerminalScrollSource::Wheel) | None => "wheel",
                };
                self.terminal(&scroll.stream_id)?
                    .scroll(direction, scroll.lines, scroll.column, scroll.row, source)
                    .await;
                return None;
            }
            ClientMessage::FileStat(request) => self.file_stat(request, device).await,
            ClientMessage::FileRead(request) => self.file_read(request, device).await,
            ClientMessage::DirectoryList(request) => self.directory_list(request).await,
            ClientMessage::SlashList(request) => self.slash_list(request).await,
            ClientMessage::PushStatus(request) => push_reply(request.request_id, self.push.status(&request.device_id).await, PushState::status_result),
            ClientMessage::PushInstall(request) => {
                let outcome = self.push.pair(&request.device_id, &request.pairing).await;
                push_reply(request.request_id, outcome, PushState::paired_result)
            }
            ClientMessage::PushUpdateMuted(request) => {
                let outcome = self.push.update_muted(&request.device_id, request.muted).await;
                push_reply(request.request_id, outcome, PushState::muted_result)
            }
            ClientMessage::PushRemove(request) => push_reply(request.request_id, self.push.unpair(&request.device_id).await, PushState::unpaired_result),
            ClientMessage::UsageCodex(request) => codex_usage_reply(request.request_id, codex_usage::read(&self.codex_cli).await),
            message @ (ClientMessage::UsageClaude(_)
            | ClientMessage::UsageSessionContext(_)
            | ClientMessage::ClaudeUsageStatus(_)
            | ClientMessage::ClaudeUsageInstall(_)
            | ClientMessage::ClaudeUsageRemove(_)) => self.claude_usage().handle(message).await,
            ClientMessage::PushTest(request) => {
                push_test_reply(request.request_id, self.push.send_test(&request.device_id, request.host_name.as_deref()).await)
            }
            ClientMessage::WorkspaceCreate(request) => structure_operations::workspace_create(&self.herdr, request).await,
            ClientMessage::WorkspaceRename(request) => structure_operations::workspace_rename(&self.herdr, request).await,
            ClientMessage::TabRename(request) => structure_operations::tab_rename(&self.herdr, request).await,
            ClientMessage::TabCreate(request) => structure_operations::tab_create(&self.herdr, request).await,
            ClientMessage::TabClose(request) => structure_operations::tab_close(&self.herdr, request).await,
            ClientMessage::WorkspaceClose(request) => structure_operations::workspace_close(&self.herdr, request).await,
            ClientMessage::WorktreeCreate(request) => structure_operations::worktree_create(&self.herdr, request).await,
            ClientMessage::AgentStart(request) => structure_operations::agent_start(&self.herdr, request, &self.transcript_environment).await,
            ClientMessage::FileUploadStart(request) => self.upload_start(request, device).await,
            ClientMessage::FileUploadChunk(request) => self.upload_chunk(request, device).await,
            ClientMessage::HostServiceStatus(request) => remote_management::status(self.service_management.clone(), request.request_id).await,
            ClientMessage::HostServiceUpdate(request) => remote_management::update(self.service_management.clone(), request.request_id).await,
            ClientMessage::HostServiceSetAutoUpdate(request) => {
                remote_management::set_auto_update(self.service_management.clone(), request.request_id, request.enabled).await
            }
            ClientMessage::DevicesList(request) => self.devices_list(request.request_id, device).await,
            ClientMessage::DevicesRevoke(request) => self.devices_revoke(request.request_id, request.device_id, device).await,
            ClientMessage::PaneTerminalClose(close) => {
                // drop:關閉 stdin(control 等同 release)並結束子行程。
                self.terminals.remove(&close.stream_id);
                return None;
            }
        };
        Some(reply)
    }

    /// 已配對(未撤銷)的裝置，不含權杖。
    async fn devices_list(&self, request_id: String, requester: &Device) -> ServerMessage {
        let devices = self.devices.clone();
        let stored_devices = match blocking(move || devices.devices()).await {
            Ok(stored_devices) => stored_devices,
            Err(storage_error) => {
                return error(
                    Some(request_id),
                    ErrorCode::InternalError,
                    &format!("cannot read device store: {storage_error}"),
                );
            }
        };
        let paired_devices = stored_devices
            .into_iter()
            .filter(|stored| stored.revoked_at.is_none())
            .map(|stored| PairedDevice {
                current: stored.device_id == requester.device_id,
                device_id: stored.device_id,
                name: stored.name,
                read_only: stored.read_only,
                paired_at: rfc3339(stored.paired_at),
            })
            .collect();
        ServerMessage::DevicesListResult(DevicesListResult {
            request_id,
            devices: paired_devices,
        })
    }

    /// 撤銷指定裝置;它的下一個訊息由每則重新驗證拒絕(4401)。
    async fn devices_revoke(&self, request_id: String, device_id: String, requester: &Device) -> ServerMessage {
        if device_id == requester.device_id {
            return error(
                Some(request_id),
                ErrorCode::InvalidMessage,
                "a device cannot revoke itself over the WebSocket; use POST /v1/revoke",
            );
        }
        let devices = self.devices.clone();
        let target_device_id = device_id.clone();
        match blocking(move || devices.revoke(&target_device_id, SystemTime::now())).await {
            Ok(_) => {
                self.audit.record(
                    AuditEvent::Revoked {
                        device_id: device_id.clone(),
                        by: RevokedBy::App,
                        by_device_id: Some(requester.device_id.clone()),
                    },
                    SystemTime::now(),
                );
                ServerMessage::DevicesRevokeResult(DevicesRevokeResult { request_id, device_id })
            }
            Err(RevokeFailure::UnknownDevice) => error(Some(request_id), ErrorCode::DeviceUnknown, "no paired device has this id"),
            Err(RevokeFailure::Storage(storage_error)) => error(
                Some(request_id),
                ErrorCode::InternalError,
                &format!("cannot update device store: {storage_error}"),
            ),
        }
    }

    /// 開著的 Pane 終端機;已關閉或不存在時略過那個訊息。
    fn terminal(&mut self, stream_id: &str) -> Option<&mut PaneTerminal> {
        self.terminals.get_mut(stream_id)
    }

    /// 以子行程執行 herdr;唯讀裝置用 observe(只能看)。
    fn open_terminal(&mut self, request: PaneTerminalOpen, device: &Device) -> ServerMessage {
        let request_id = request.request_id.clone();
        if !pane_terminal::is_valid_target(&request.target) || !pane_terminal::is_valid_size(request.columns, request.rows) {
            return error(Some(request_id), ErrorCode::InvalidMessage, "invalid terminal target or size");
        }
        if self.terminals.contains_key(&request_id) {
            return error(Some(request_id), ErrorCode::InvalidMessage, "stream is already open");
        }
        let observe = device.read_only;
        let terminal = match PaneTerminal::spawn(&self.herdr_cli, &request, observe, self.terminal_frames.clone()) {
            Ok(terminal) => terminal,
            Err(spawn_error) => return error(Some(request_id), ErrorCode::HerdrUnavailable, &format!("cannot run herdr: {spawn_error}")),
        };
        if !observe {
            // 接上 Pane 是寫入類操作(會改 Pane 尺寸、可能接管):記一筆，不記之後的按鍵。
            self.audit.record(
                AuditEvent::Write {
                    device_id: device.device_id.clone(),
                    operation: "pane.terminal.open".to_owned(),
                    workspace_id: None,
                    pane_id: Some(request.target.clone()),
                    allowed: true,
                },
                SystemTime::now(),
            );
        }
        self.terminals.insert(request_id.clone(), terminal);
        ServerMessage::PaneTerminalOpenResult(PaneTerminalOpenResult {
            request_id: request_id.clone(),
            stream_id: request_id,
            read_only: observe,
        })
    }

    fn queue_list(&self, request: QueueList) -> ServerMessage {
        let Some(observer) = &self.observer else {
            return observer_missing(request.request_id);
        };
        ServerMessage::QueueListResult(QueueListResult {
            queues: observer.list(request.pane_id.as_deref()),
            request_id: request.request_id,
        })
    }

    fn queue_add(&self, request: QueueAdd) -> ServerMessage {
        let Some(observer) = &self.observer else {
            return observer_missing(request.request_id);
        };
        if !pane_terminal::is_valid_target(&request.pane_id) || request.text.len() > MAXIMUM_PROMPT_BYTES {
            return error(
                Some(request.request_id),
                ErrorCode::InvalidMessage,
                &format!("invalid pane_id or text larger than {} KiB", MAXIMUM_PROMPT_BYTES / 1024),
            );
        }
        match observer.add(&request.pane_id, request.text, request.expected_session) {
            Ok(item_id) => ServerMessage::QueueAddResult(QueueAddResult {
                request_id: request.request_id,
                item_id,
            }),
            Err(failure) => queue_error(request.request_id, failure),
        }
    }

    fn queue_remove(&self, request: QueueRemove) -> ServerMessage {
        let Some(observer) = &self.observer else {
            return observer_missing(request.request_id);
        };
        match observer.remove(&request.pane_id, &request.item_id) {
            Ok(()) => ServerMessage::QueueRemoveResult(QueueRemoveResult {
                request_id: request.request_id,
            }),
            Err(failure) => queue_error(request.request_id, failure),
        }
    }

    fn queue_reorder(&self, request: QueueReorder) -> ServerMessage {
        let Some(observer) = &self.observer else {
            return observer_missing(request.request_id);
        };
        match observer.reorder(&request.pane_id, &request.item_ids) {
            Ok(()) => ServerMessage::QueueReorderResult(QueueReorderResult {
                request_id: request.request_id,
            }),
            Err(failure) => queue_error(request.request_id, failure),
        }
    }

    /// 工作資料夾過去的 session(在 blocking 執行緒讀檔)。
    async fn sessions_list(&self, request: SessionsList) -> ServerMessage {
        if !std::path::Path::new(&request.cwd).is_absolute() {
            return error(Some(request.request_id), ErrorCode::InvalidMessage, "cwd must be an absolute path");
        }
        let environment = self.transcript_environment.clone();
        let sessions = blocking(move || session_history::list(&environment, &request.cwd, request.agent_kind)).await;
        ServerMessage::SessionsListResult(SessionsListResult {
            request_id: request.request_id,
            sessions,
        })
    }

    /// git.status／git.diff 只讀 herdr 目前回報的 Agent 工作目錄。Err 是要回的錯誤。
    async fn reported_working_directory(&self, request_id: &str, cwd: &str) -> Result<PathBuf, Box<ServerMessage>> {
        let result = match self.herdr.request("session.snapshot", json!({})).await {
            Ok(result) => result,
            Err(failure) => return Err(Box::new(herdr_error(request_id.to_owned(), "session.snapshot", failure))),
        };
        let snapshot = serde_json::from_value::<SnapshotResult>(result).unwrap_or_default().snapshot;
        if snapshot.agents.iter().any(|agent| agent.cwd.as_deref() == Some(cwd)) {
            Ok(PathBuf::from(cwd))
        } else {
            Err(Box::new(error(
                Some(request_id.to_owned()),
                ErrorCode::CwdNotAllowed,
                "cwd is not the working directory of an agent herdr reports",
            )))
        }
    }

    async fn git_status(&self, request: GitStatus) -> ServerMessage {
        let cwd = match self.reported_working_directory(&request.request_id, &request.cwd).await {
            Ok(cwd) => cwd,
            Err(reply) => return *reply,
        };
        match blocking(move || git_changes::status(&cwd)).await {
            Ok(changes) => ServerMessage::GitStatusResult(GitStatusResult {
                request_id: request.request_id,
                branch: changes.branch,
                files: changes.files,
            }),
            Err(failure) => git_error(request.request_id, failure),
        }
    }

    async fn git_diff(&self, request: GitDiff) -> ServerMessage {
        let cwd = match self.reported_working_directory(&request.request_id, &request.cwd).await {
            Ok(cwd) => cwd,
            Err(reply) => return *reply,
        };
        let only = request.path.clone();
        match blocking(move || git_changes::diff(&cwd, only.as_deref())).await {
            Ok((files, truncated)) => ServerMessage::GitDiffResult(GitDiffResult {
                request_id: request.request_id,
                files,
                truncated,
            }),
            Err(failure) => git_error(request.request_id, failure),
        }
    }

    async fn pane_send_text(&self, request: PaneSendText) -> ServerMessage {
        if let Err(failure) = pane_guard::check(
            &self.herdr,
            &request.pane_id,
            request.expected_session.as_deref(),
            request.expected_screen.as_deref(),
        )
        .await
        {
            return guard_error(request.request_id, failure);
        }
        let params = json!({"pane_id": request.pane_id, "text": request.text});
        match self.herdr.request("pane.send_text", params).await {
            Ok(_) => ServerMessage::PaneSendTextResult(PaneSendTextResult {
                request_id: request.request_id,
            }),
            Err(failure) => herdr_error(request.request_id, "pane.send_text", failure),
        }
    }

    async fn agent_prompt(&self, request: AgentPrompt) -> ServerMessage {
        if let Err(failure) = pane_guard::check(
            &self.herdr,
            &request.pane_id,
            request.expected_session.as_deref(),
            request.expected_screen.as_deref(),
        )
        .await
        {
            return guard_error(request.request_id, failure);
        }
        let mut params = json!({"target": request.pane_id, "text": request.text});
        let mut herdr_wait = Duration::ZERO;
        let waited = request.wait.is_some();
        if let Some(wait) = request.wait {
            herdr_wait = Duration::from_millis(u64::try_from(wait.timeout_ms).unwrap_or(0));
            params["wait"] = json!({"until": wait.until, "timeout_ms": wait.timeout_ms});
        }
        match self.herdr.request_waiting("agent.prompt", params, herdr_wait).await {
            Ok(result) => {
                let delivery = result["delivery"].as_str().map(str::to_owned);
                let mut reply = AgentPromptResult {
                    request_id: request.request_id,
                    delivery,
                    submit_confirmed: None,
                    enter_sent: None,
                };
                // 只有 prompt 路徑(帶 wait)才確認;slash／`!` 指令不帶 wait，不等也不補 Enter。
                if waited && reply.delivery.as_deref() == Some("written_to_pty") {
                    self.confirm_written_prompt(&request.pane_id, request.expected_session.as_deref(), &result, &mut reply)
                        .await;
                }
                ServerMessage::AgentPromptResult(reply)
            }
            Err(failure) => herdr_error(request.request_id, "agent.prompt", failure),
        }
    }

    /// herdr 只回報文字寫進終端機時，照 SSH 的規則確認(raychiutw/purrdr.host#107):等 Agent 進入 working／blocked,
    /// 沒反應就重讀狀態，只有 idle／done(且 session 沒換)才補一個 Enter,再等一次。Codex 沒有送達回條，不處理。
    async fn confirm_written_prompt(&self, pane_id: &str, expected_session: Option<&str>, result: &Value, reply: &mut AgentPromptResult) {
        let agent = &result["agent"];
        if agent["agent"].as_str().is_none_or(|kind| kind == "codex") {
            return;
        }
        match agent["agent_status"].as_str() {
            // 原本就在工作(prompt 只會排隊)或已停下來問問題:不用等。
            Some("working" | "blocked") => {
                reply.submit_confirmed = Some(true);
                return;
            }
            Some("idle" | "done") => {}
            _ => return,
        }
        reply.submit_confirmed = Some(false);
        match self.wait_for_reaction(pane_id, CONFIRM_WAIT).await {
            Ok(true) => {
                reply.submit_confirmed = Some(true);
                return;
            }
            Ok(false) => {}
            Err(_) => return,
        }
        let Ok(snapshot) = self.herdr.request("session.snapshot", json!({})).await else {
            return;
        };
        let status = snapshot["snapshot"]["agents"]
            .as_array()
            .and_then(|agents| agents.iter().find(|agent| agent["pane_id"] == pane_id))
            .and_then(|agent| agent["agent_status"].as_str());
        let session_matches = expected_session.is_none_or(|expected| pane_guard::session_of(&snapshot, pane_id).as_deref() == Some(expected));
        if !matches!(status, Some("idle" | "done")) || !session_matches {
            return;
        }
        if self
            .herdr
            .request("pane.send_keys", json!({"pane_id": pane_id, "keys": ["enter"]}))
            .await
            .is_err()
        {
            return;
        }
        reply.enter_sent = Some(true);
        reply.submit_confirmed = Some(matches!(self.wait_for_reaction(pane_id, ENTER_WAIT).await, Ok(true)));
    }

    /// `agent.wait` 等 Agent 進入 working／blocked;herdr 回 `timeout` 是 `Ok(false)`。
    async fn wait_for_reaction(&self, pane_id: &str, timeout: Duration) -> Result<bool, HerdrFailure> {
        let params = json!({"target": pane_id, "until": ["working", "blocked"], "timeout_ms": timeout.as_millis()});
        match self.herdr.request_waiting("agent.wait", params, timeout).await {
            Ok(_) => Ok(true),
            Err(HerdrFailure::Herdr { code, .. }) if code == "timeout" => Ok(false),
            Err(failure) => Err(failure),
        }
    }

    /// 開 herdr 訂閱;herdr 接受後先回結果，再開始轉送事件(順序不會顛倒)。
    async fn events_subscribe(&mut self, request: EventsSubscribe) -> ServerMessage {
        if self.subscriptions.contains_key(&request.stream_id) {
            return error(
                Some(request.request_id),
                ErrorCode::InvalidMessage,
                "stream_id is already used on this connection",
            );
        }
        let subscriptions: Vec<Value> = request
            .subscriptions
            .iter()
            .map(|subscription| match &subscription.pane_id {
                Some(pane_id) => json!({"type": subscription.event, "pane_id": pane_id}),
                None => json!({"type": subscription.event}),
            })
            .collect();
        let subscription = match self.herdr.subscribe(json!({"subscriptions": subscriptions})).await {
            Ok(subscription) => subscription,
            Err(failure) => return herdr_error(request.request_id, "events.subscribe", failure),
        };
        self.accepted_subscription = Some((request.stream_id.clone(), subscription));
        ServerMessage::EventsSubscribeResult(EventsSubscribeResult {
            request_id: request.request_id,
            stream_id: request.stream_id,
        })
    }

    /// 轉送 herdr 事件;herdr 結束訂閱時送 `events.ended`(失敗時帶錯誤碼)。
    fn start_forwarding(&mut self, stream_id: String, mut subscription: HerdrSubscription) {
        let outbound = self.outbound.clone();
        let forwarded_stream_id = stream_id.clone();
        let forwarding = tokio::spawn(async move {
            let mut ended = EventsEnded {
                stream_id: forwarded_stream_id.clone(),
                code: None,
                message: None,
                herdr_code: None,
            };
            loop {
                match subscription.next_event().await {
                    Ok(Some(event)) => {
                        let _ = outbound.send(Outbound::Message(ServerMessage::SubscribedEvent(SubscribedEvent {
                            stream_id: forwarded_stream_id.clone(),
                            event: event["event"].as_str().unwrap_or_default().to_owned(),
                            // 有些事件的 pane id 在 data 頂層，有些包在 pane 物件內。
                            pane_id: event["data"]["pane_id"]
                                .as_str()
                                .or_else(|| event["data"]["pane"]["pane_id"].as_str())
                                .map(str::to_owned),
                        })));
                    }
                    Ok(None) => break,
                    Err(HerdrFailure::Herdr { code, message }) => {
                        ended.code = Some(ErrorCode::HerdrError);
                        ended.message = Some(message);
                        ended.herdr_code = Some(code);
                        break;
                    }
                    Err(HerdrFailure::Unavailable(message) | HerdrFailure::UnexpectedResponse(message)) => {
                        ended.code = Some(ErrorCode::HerdrUnavailable);
                        ended.message = Some(message);
                        break;
                    }
                }
            }
            let _ = outbound.send(Outbound::Message(ServerMessage::EventsEnded(ended)));
        });
        self.subscriptions.insert(stream_id, forwarding);
    }

    /// 主機取檔的規則:herdr snapshot 目前回報的工作目錄、這條連線定位過的對話紀錄與主機服務的環境。
    async fn file_access_rules(&self) -> Result<FileAccessRules, HerdrFailure> {
        let result = self.herdr.request("session.snapshot", json!({})).await?;
        let snapshot = serde_json::from_value::<SnapshotResult>(result).unwrap_or_default().snapshot;
        Ok(FileAccessRules {
            working_directories: snapshot.agents.into_iter().filter_map(|agent| agent.cwd).map(PathBuf::from).collect(),
            home: self.files.home.clone(),
            temporary_directories: self.files.temporary_directory.iter().cloned().collect(),
            transcripts: self.transcripts.located_paths(),
            excluded_directories: vec![self.files.state_directory.clone()],
        })
    }

    /// 套用存取規則並開檔(在 blocking 執行緒);超過頻率上限時先等。
    async fn locate_file(&mut self, path: &str, purpose: Purpose) -> Result<Located, HerdrFailure> {
        let delay = self.file_pacing.delay_for(std::time::Instant::now());
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let rules = self.file_access_rules().await?;
        let cache = self.file_cache.clone();
        let path = path.to_owned();
        Ok(blocking(move || {
            let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            host_files::locate(&path, &rules, &mut cache, purpose)
        })
        .await)
    }

    fn audit_file_access(&self, device: &Device, operation: &'static str, path: &str, result: String, size: Option<u64>) {
        self.audit.record(
            AuditEvent::FileAccess {
                device_id: device.device_id.clone(),
                operation,
                path: path.to_owned(),
                result,
                size,
            },
            SystemTime::now(),
        );
    }

    /// 只回子資料夾名稱;排除與 symlink 規則在 `directory_list`。
    async fn directory_list(&self, request: DirectoryList) -> ServerMessage {
        let path = request.path.clone();
        let state_directory = self.files.state_directory.clone();
        match blocking(move || directory_list::list(&path, &state_directory)).await {
            Ok(listing) => ServerMessage::DirectoryListResult(DirectoryListResult {
                request_id: request.request_id,
                path: request.path,
                directories: listing.directories,
                truncated: listing.truncated,
            }),
            Err(ListError::InvalidPath) => error(
                Some(request.request_id),
                ErrorCode::InvalidMessage,
                "path must be an absolute path without . or .. segments",
            ),
            Err(ListError::NotFound) => error(
                Some(request.request_id),
                ErrorCode::DirectoryNotFound,
                "path is not an existing folder on this host",
            ),
            Err(ListError::NotAllowed) => error(
                Some(request.request_id),
                ErrorCode::DirectoryNotAllowed,
                "this folder is excluded from browsing",
            ),
            Err(ListError::Unreadable) => error(Some(request.request_id), ErrorCode::DirectoryUnreadable, "cannot read this folder"),
        }
    }

    /// Claude 的 slash 清單:只讀主機服務的家目錄(`files.home`)底下的 `.claude`,規則在 `slash_list`。
    async fn slash_list(&self, request: crate::protocol::SlashList) -> ServerMessage {
        let home = self.files.home.clone();
        let state_directory = self.files.state_directory.clone();
        let listing = blocking(move || crate::slash_list::list(home.as_deref(), &state_directory)).await;
        ServerMessage::SlashListResult(crate::protocol::SlashListResult {
            request_id: request.request_id,
            commands: listing.commands,
            disabled_plugins: listing.disabled_plugins,
            truncated: listing.truncated,
        })
    }

    async fn file_stat(&mut self, request: FileStat, device: &Device) -> ServerMessage {
        let located = match self.locate_file(&request.path, Purpose::Status).await {
            Ok(located) => located,
            Err(failure) => return herdr_error(request.request_id, "session.snapshot", failure),
        };
        self.audit_file_access(device, "file.stat", &request.path, status_name(located.status()), located.size());
        ServerMessage::FileStatResult(FileStatResult {
            request_id: request.request_id,
            status: located.status(),
            version: located.version().map(str::to_owned),
            size: located.size().map(|size| size as i64),
        })
    }

    /// 從頭讀或還沒有這個版本的快照時重新檢查並建快照;之後的分塊從快照讀。
    async fn file_read(&mut self, request: FileRead, device: &Device) -> ServerMessage {
        let (Ok(offset), Ok(length)) = (u64::try_from(request.offset), u64::try_from(request.length)) else {
            return error(Some(request.request_id), ErrorCode::InvalidMessage, "offset and length must not be negative");
        };
        let reusable = self
            .file_snapshot
            .as_ref()
            .is_some_and(|snapshot| offset > 0 && snapshot.path == request.path && snapshot.version == request.version);
        if !reusable {
            let located = match self.locate_file(&request.path, Purpose::Contents).await {
                Ok(located) => located,
                Err(failure) => return herdr_error(request.request_id, "session.snapshot", failure),
            };
            let (contents, version) = match located {
                Located::File { contents, version, .. } => (contents.unwrap_or_default(), version),
                Located::Unavailable { status, size } => {
                    self.audit_file_access(device, "file.read", &request.path, status_name(status), size);
                    return ServerMessage::FileReadResult(FileReadResult {
                        request_id: request.request_id,
                        status,
                        size: size.map(|size| size as i64),
                        offset: None,
                        bytes: None,
                    });
                }
            };
            let changed = version != request.version;
            let result = if changed { "changed".to_owned() } else { status_name(FileStatus::File) };
            self.audit_file_access(device, "file.read", &request.path, result, Some(contents.len() as u64));
            self.file_snapshot = Some(FileSnapshot {
                path: request.path.clone(),
                version,
                contents,
            });
            if changed {
                return error(Some(request.request_id), ErrorCode::FileChanged, "file changed since its status was read");
            }
        }
        let snapshot = self.file_snapshot.as_ref().expect("剛建好或可沿用的快照");
        ServerMessage::FileReadResult(FileReadResult {
            request_id: request.request_id,
            status: FileStatus::File,
            size: None,
            offset: Some(offset as i64),
            bytes: Some(BASE64.encode(snapshot.chunk(offset, length))),
        })
    }

    async fn upload_start(&mut self, request: FileUploadStart, device: &Device) -> ServerMessage {
        let Some(uploads) = self.uploads.clone() else {
            return error(
                Some(request.request_id),
                ErrorCode::InternalError,
                "host service cannot find the home directory",
            );
        };
        let Ok(size) = u64::try_from(request.size) else {
            return error(Some(request.request_id), ErrorCode::InvalidMessage, "size must not be negative");
        };
        let extension = request.extension.clone();
        match blocking(move || uploads.begin(size, extension.as_deref())).await {
            Ok(pending) => {
                self.pending_uploads.insert(request.request_id.clone(), pending);
                ServerMessage::FileUploadStartResult(FileUploadStartResult {
                    upload_id: request.request_id.clone(),
                    request_id: request.request_id,
                })
            }
            Err(failure) => {
                self.audit_upload(device, None, size, failure.audit_result());
                upload_error(request.request_id, failure)
            }
        }
    }

    fn audit_upload(&self, device: &Device, path: Option<String>, size: u64, result: &'static str) {
        self.audit.record(
            AuditEvent::Upload {
                device_id: device.device_id.clone(),
                path,
                size,
                result,
            },
            SystemTime::now(),
        );
    }

    /// 依序寫入分塊;失敗時這次上傳作廢(drop 時刪掉暫存檔)。
    async fn upload_chunk(&mut self, request: FileUploadChunk, device: &Device) -> ServerMessage {
        let Some(mut pending) = self.pending_uploads.remove(&request.upload_id) else {
            return error(Some(request.request_id), ErrorCode::InvalidMessage, "unknown upload_id");
        };
        let Ok(bytes) = BASE64.decode(&request.bytes) else {
            return error(Some(request.request_id), ErrorCode::InvalidMessage, "bytes is not base64");
        };
        let offset = u64::try_from(request.offset).unwrap_or(u64::MAX);
        let (pending, outcome) = blocking(move || {
            let outcome = pending.append(offset, &bytes);
            (pending, outcome)
        })
        .await;
        match outcome {
            Ok(ChunkOutcome::Partial { received }) => {
                self.pending_uploads.insert(request.upload_id.clone(), pending);
                ServerMessage::FileUploadChunkResult(FileUploadChunkResult {
                    request_id: request.request_id,
                    upload_id: request.upload_id,
                    received: received as i64,
                    path: None,
                })
            }
            Ok(ChunkOutcome::Stored { received, path }) => {
                let path = path.to_string_lossy().into_owned();
                self.audit_upload(device, Some(path.clone()), pending.size(), "stored");
                ServerMessage::FileUploadChunkResult(FileUploadChunkResult {
                    request_id: request.request_id,
                    upload_id: request.upload_id,
                    received: received as i64,
                    path: Some(path),
                })
            }
            Err(failure) => {
                self.audit_upload(device, None, pending.size(), failure.audit_result());
                upload_error(request.request_id, failure)
            }
        }
    }

    /// 對話紀錄只能依 herdr 目前回報的 agent_session 定位(#59)。
    async fn transcript_locate(&mut self, request: TranscriptLocate) -> ServerMessage {
        let result = match self.herdr.request("session.snapshot", json!({})).await {
            Ok(result) => result,
            Err(failure) => return herdr_error(request.request_id, "session.snapshot", failure),
        };
        let snapshot = serde_json::from_value::<SnapshotResult>(result).unwrap_or_default().snapshot;
        let reported = snapshot.agents.iter().find(|agent| {
            agent
                .agent_session
                .as_ref()
                .is_some_and(|session| session.kind == request.session.kind && session.value == request.session.value)
        });
        let Some(agent) = reported else {
            return error(Some(request.request_id), ErrorCode::TranscriptNotAllowed, "herdr does not report this session");
        };
        let session = AgentSessionInfo {
            agent: agent.agent.clone().unwrap_or_default(),
            cwd: agent.cwd.clone().unwrap_or_default(),
            kind: request.session.kind,
            value: request.session.value,
        };
        self.transcripts.locate(request.request_id, session, request.header).await
    }

    async fn pane_send_keys(&self, request: PaneSendKeys) -> ServerMessage {
        if let Err(failure) = pane_guard::check(
            &self.herdr,
            &request.pane_id,
            request.expected_session.as_deref(),
            request.expected_screen.as_deref(),
        )
        .await
        {
            return guard_error(request.request_id, failure);
        }
        let params = json!({"pane_id": request.pane_id, "keys": request.keys});
        match self.herdr.request("pane.send_keys", params).await {
            Ok(_) => ServerMessage::PaneSendKeysResult(PaneSendKeysResult {
                request_id: request.request_id,
            }),
            Err(failure) => herdr_error(request.request_id, "pane.send_keys", failure),
        }
    }

    async fn hello(&mut self, request_id: String, protocol_version: i64, read_only: bool) -> ServerMessage {
        if protocol_version != PROTOCOL_VERSION {
            let mut unsupported = error_message(
                Some(request_id),
                ErrorCode::ProtocolVersionUnsupported,
                &format!("protocol version {protocol_version} is not supported"),
            );
            unsupported.supported_protocol_versions = Some(vec![PROTOCOL_VERSION]);
            return ServerMessage::ErrorMessage(unsupported);
        }
        let pong = match self.herdr.request("ping", json!({})).await {
            Ok(pong) => pong,
            Err(failure) => return herdr_error(request_id, "ping", failure),
        };
        let (Some(version), Some(protocol)) = (pong["version"].as_str(), pong["protocol"].as_i64()) else {
            return error(Some(request_id), ErrorCode::HerdrError, "herdr ping response has no version or protocol");
        };
        let mut capabilities: Vec<String> = pong["capabilities"]
            .as_object()
            .map(|flags| {
                flags
                    .iter()
                    .filter(|(_, enabled)| enabled.as_bool() == Some(true))
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default();
        capabilities.sort();
        self.handshaken = true;
        ServerMessage::Welcome(Welcome {
            request_id,
            protocol_version: PROTOCOL_VERSION,
            host_service_version: env!("CARGO_PKG_VERSION").to_owned(),
            herdr: HerdrServer {
                version: version.to_owned(),
                protocol,
                capabilities,
            },
            read_only,
            home_directory: self.files.home.as_ref().map(|home| home.to_string_lossy().into_owned()),
        })
    }

    async fn agents_list(&self, request_id: String) -> ServerMessage {
        match self.herdr.request("session.snapshot", json!({})).await {
            Ok(result) => ServerMessage::AgentsListResult(agents_list_result(request_id, result)),
            Err(failure) => herdr_error(request_id, "session.snapshot", failure),
        }
    }
}

/// herdr `session.snapshot` 的結果裡用到的部分(herdr 的 snake_case 原樣)。
/// 跟 app 端一樣寬鬆：欄位缺少或格式不對時用預設值，不讓整個列表失敗。
#[derive(Deserialize, Default)]
struct SnapshotResult {
    #[serde(default, deserialize_with = "lenient_value")]
    snapshot: HerdrSnapshot,
}

#[derive(Deserialize, Default)]
struct HerdrSnapshot {
    #[serde(default, deserialize_with = "lenient_list")]
    workspaces: Vec<HerdrWorkspace>,
    #[serde(default, deserialize_with = "lenient_list")]
    tabs: Vec<HerdrTab>,
    #[serde(default, deserialize_with = "lenient_list")]
    panes: Vec<HerdrPane>,
    #[serde(default, deserialize_with = "lenient_list")]
    layouts: Vec<HerdrLayout>,
    #[serde(default, deserialize_with = "lenient_list")]
    agents: Vec<HerdrAgent>,
}

#[derive(Deserialize, Default)]
struct HerdrTab {
    #[serde(default, deserialize_with = "lenient_value")]
    tab_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    workspace_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    label: String,
}

#[derive(Deserialize, Default)]
struct HerdrPane {
    #[serde(default, deserialize_with = "lenient_value")]
    pane_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    terminal_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    workspace_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    tab_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    focused: bool,
    #[serde(default, deserialize_with = "lenient_value")]
    cwd: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    foreground_cwd: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    agent: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    agent_status: Option<AgentStatus>,
    #[serde(default, deserialize_with = "lenient_value")]
    terminal_title: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    terminal_title_stripped: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    agent_session: Option<AgentSession>,
}

#[derive(Deserialize, Default)]
struct HerdrLayout {
    #[serde(default, deserialize_with = "lenient_value")]
    workspace_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    tab_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    zoomed: bool,
    #[serde(default, deserialize_with = "lenient_value")]
    area: HerdrRect,
    #[serde(default, deserialize_with = "lenient_value")]
    focused_pane_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    panes: Vec<HerdrLayoutPane>,
    #[serde(default, deserialize_with = "lenient_list")]
    splits: Vec<HerdrLayoutSplit>,
}

#[derive(Deserialize, Default)]
struct HerdrLayoutPane {
    #[serde(default, deserialize_with = "lenient_value")]
    pane_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    focused: bool,
    #[serde(default, deserialize_with = "lenient_value")]
    rect: HerdrRect,
}

#[derive(Deserialize, Default)]
struct HerdrLayoutSplit {
    #[serde(default, deserialize_with = "lenient_value")]
    id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    direction: String,
    #[serde(default, deserialize_with = "lenient_value")]
    ratio: f64,
    #[serde(default, deserialize_with = "lenient_value")]
    rect: HerdrRect,
}

#[derive(Deserialize, Default)]
struct HerdrRect {
    #[serde(default, deserialize_with = "lenient_value")]
    x: i64,
    #[serde(default, deserialize_with = "lenient_value")]
    y: i64,
    #[serde(default, deserialize_with = "lenient_value")]
    width: i64,
    #[serde(default, deserialize_with = "lenient_value")]
    height: i64,
}

impl From<HerdrRect> for LayoutRect {
    fn from(rect: HerdrRect) -> Self {
        LayoutRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

#[derive(Deserialize, Default)]
struct HerdrWorkspace {
    #[serde(default, deserialize_with = "lenient_value")]
    workspace_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    label: String,
}

#[derive(Deserialize, Default)]
struct HerdrAgent {
    #[serde(default, deserialize_with = "lenient_value")]
    pane_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    workspace_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    tab_id: String,
    #[serde(default, deserialize_with = "lenient_value")]
    agent: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    agent_status: Option<AgentStatus>,
    #[serde(default, deserialize_with = "lenient_value")]
    terminal_title_stripped: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    cwd: Option<String>,
    #[serde(default, deserialize_with = "lenient_value")]
    agent_session: Option<AgentSession>,
}

/// 格式不對的值當成沒有(預設值)。
fn lenient_value<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(deserializer: D) -> Result<T, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(deserializer)?).unwrap_or_default())
}

/// 不是陣列當成空的;陣列裡格式不對的項目用預設值。
fn lenient_list<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(deserializer: D) -> Result<Vec<T>, D::Error> {
    let items = match Value::deserialize(deserializer)? {
        Value::Array(items) => items,
        _ => Vec::new(),
    };
    Ok(items.into_iter().map(|item| serde_json::from_value(item).unwrap_or_default()).collect())
}

fn agents_list_result(request_id: String, result: Value) -> AgentsListResult {
    let snapshot = serde_json::from_value::<SnapshotResult>(result).unwrap_or_default().snapshot;
    let workspaces = snapshot
        .workspaces
        .into_iter()
        .map(|workspace| Workspace {
            workspace_id: workspace.workspace_id,
            label: workspace.label,
        })
        .collect();
    let tabs = snapshot
        .tabs
        .into_iter()
        .map(|tab| Tab {
            tab_id: tab.tab_id,
            workspace_id: tab.workspace_id,
            label: tab.label,
        })
        .collect();
    let panes = snapshot
        .panes
        .into_iter()
        .map(|pane| Pane {
            pane_id: pane.pane_id,
            terminal_id: pane.terminal_id,
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            focused: pane.focused,
            cwd: pane.cwd,
            foreground_cwd: pane.foreground_cwd,
            agent: pane.agent,
            agent_status: pane.agent_status.unwrap_or(AgentStatus::Unknown),
            terminal_title: pane.terminal_title,
            terminal_title_stripped: pane.terminal_title_stripped,
            agent_session: pane.agent_session,
        })
        .collect();
    let layouts = snapshot
        .layouts
        .into_iter()
        .map(|layout| Layout {
            workspace_id: layout.workspace_id,
            tab_id: layout.tab_id,
            zoomed: layout.zoomed,
            area: layout.area.into(),
            focused_pane_id: layout.focused_pane_id,
            panes: layout
                .panes
                .into_iter()
                .map(|pane| LayoutPane {
                    pane_id: pane.pane_id,
                    focused: pane.focused,
                    rect: pane.rect.into(),
                })
                .collect(),
            splits: layout
                .splits
                .into_iter()
                .map(|split| LayoutSplit {
                    id: split.id,
                    direction: split.direction,
                    ratio: split.ratio,
                    rect: split.rect.into(),
                })
                .collect(),
        })
        .collect();
    let agents = snapshot
        .agents
        .into_iter()
        .map(|agent| Agent {
            pane_id: agent.pane_id,
            workspace_id: agent.workspace_id,
            tab_id: agent.tab_id,
            agent: agent.agent,
            status: agent.agent_status.unwrap_or(AgentStatus::Unknown),
            title: agent.terminal_title_stripped,
            cwd: agent.cwd,
            session: agent.agent_session,
        })
        .collect();
    AgentsListResult {
        request_id,
        workspaces,
        tabs: Some(tabs),
        panes: Some(panes),
        layouts: Some(layouts),
        agents,
    }
}

pub(crate) fn herdr_error(request_id: String, method: &str, failure: HerdrFailure) -> ServerMessage {
    if failure.is_unknown_method(method) {
        return error(
            Some(request_id),
            ErrorCode::HerdrCapabilityMissing,
            &format!("herdr server does not support {method}"),
        );
    }
    match failure {
        HerdrFailure::Unavailable(message) => error(Some(request_id), ErrorCode::HerdrUnavailable, &message),
        HerdrFailure::UnexpectedResponse(message) => error(Some(request_id), ErrorCode::HerdrError, &message),
        HerdrFailure::Herdr { code, message } => {
            let mut herdr_error = error_message(Some(request_id), ErrorCode::HerdrError, &message);
            herdr_error.herdr_code = Some(code);
            ServerMessage::ErrorMessage(herdr_error)
        }
    }
}

/// 送出前比對不通過時的錯誤：對象變了回 pane_changed,讀不到 herdr 照一般 herdr 錯誤。
pub(crate) fn guard_error(request_id: String, failure: GuardFailure) -> ServerMessage {
    match failure {
        GuardFailure::Changed(message) => error(Some(request_id), ErrorCode::PaneChanged, &message),
        GuardFailure::Herdr { method, failure } => herdr_error(request_id, method, failure),
    }
}

/// 佇列變更推給這條連線(跟不上而漏掉的就略過，app 可再 queue.list)。
fn forward_queue_changes(mut changes: tokio::sync::broadcast::Receiver<ServerMessage>, outbound: mpsc::UnboundedSender<Outbound>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match changes.recv().await {
                Ok(change) => {
                    if outbound.send(Outbound::Message(change)).is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

fn observer_missing(request_id: String) -> ServerMessage {
    error(Some(request_id), ErrorCode::InternalError, "the agent observer is not running")
}

fn queue_error(request_id: String, failure: QueueFailure) -> ServerMessage {
    match failure {
        QueueFailure::Full => error(
            Some(request_id),
            ErrorCode::QueueFull,
            &format!("this pane already has {MAXIMUM_QUEUED_PROMPTS} queued prompts"),
        ),
        QueueFailure::UnknownItem => error(Some(request_id), ErrorCode::QueueItemUnknown, "item_id is not in this queue"),
        QueueFailure::Storage(message) => error(Some(request_id), ErrorCode::InternalError, &message),
    }
}

fn git_error(request_id: String, failure: GitFailure) -> ServerMessage {
    match failure {
        GitFailure::NotRepository => error(Some(request_id), ErrorCode::NotGitRepository, "cwd is not inside a git working tree"),
        GitFailure::Read(message) => error(Some(request_id), ErrorCode::InternalError, &message),
    }
}

/// `FileStatus` 的協定名稱(稽核紀錄用)。
fn status_name(status: FileStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn error_message(request_id: Option<String>, code: ErrorCode, message: &str) -> ErrorMessage {
    ErrorMessage {
        request_id,
        code,
        message: message.to_owned(),
        herdr_code: None,
        supported_protocol_versions: None,
    }
}

/// 推播外掛代裝的結果轉成回應或錯誤。
fn push_reply(request_id: String, outcome: Result<PushState, PushFailure>, result: impl FnOnce(String, PushState) -> ServerMessage) -> ServerMessage {
    match outcome {
        Ok(state) => result(request_id, state),
        Err(failure) => push_error(request_id, failure),
    }
}

/// 推播請求失敗的錯誤回應。
fn push_error(request_id: String, failure: PushFailure) -> ServerMessage {
    match failure {
        PushFailure::InvalidRequest(message) => error(Some(request_id), ErrorCode::InvalidMessage, message),
        PushFailure::HerdrUnavailable(message) => error(Some(request_id), ErrorCode::HerdrUnavailable, &message),
        PushFailure::Herdr(message) => error(Some(request_id), ErrorCode::PushPluginFailed, &message),
        PushFailure::NotPaired => error(Some(request_id), ErrorCode::PushNotPaired, "this device has no push pairing on this host"),
        PushFailure::SendFailed(message) => error(Some(request_id), ErrorCode::PushSendFailed, &message),
        PushFailure::Internal(message) => error(Some(request_id), ErrorCode::InternalError, &message),
    }
}

/// Codex 方案用量的結果轉成回應或錯誤。
fn codex_usage_reply(request_id: String, outcome: Result<codex_usage::CodexUsage, CodexUsageFailure>) -> ServerMessage {
    match outcome {
        Ok(usage) => ServerMessage::UsageCodexResult(UsageCodexResult {
            request_id,
            captured_at: usage.captured_at,
            periods: usage.periods,
        }),
        Err(CodexUsageFailure::NotFound) => error(Some(request_id), ErrorCode::CodexNotFound, "codex was not found on this host"),
        Err(CodexUsageFailure::NotSignedIn) => error(Some(request_id), ErrorCode::CodexNotSignedIn, "Codex is not signed in with ChatGPT"),
        Err(CodexUsageFailure::Failed(message)) => error(Some(request_id), ErrorCode::CodexFailed, &message),
    }
}

/// 測試通知的結果轉成回應或錯誤。
fn push_test_reply(request_id: String, outcome: Result<(), PushFailure>) -> ServerMessage {
    match outcome {
        Ok(()) => ServerMessage::PushTestResult(PushTestResult { request_id }),
        Err(failure) => push_error(request_id, failure),
    }
}

fn upload_error(request_id: String, failure: UploadFailure) -> ServerMessage {
    match failure {
        UploadFailure::TooLarge(message) => error(Some(request_id), ErrorCode::UploadTooLarge, &message),
        UploadFailure::Invalid(message) => error(Some(request_id), ErrorCode::InvalidMessage, message),
        UploadFailure::Storage(storage_error) => error(Some(request_id), ErrorCode::InternalError, &format!("cannot write the upload: {storage_error}")),
    }
}

pub(crate) fn error(request_id: Option<String>, code: ErrorCode, message: &str) -> ServerMessage {
    ServerMessage::ErrorMessage(error_message(request_id, code, message))
}
