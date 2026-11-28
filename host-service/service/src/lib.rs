//! Purrdr 主機服務(ADR-0010)。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

pub mod agent_activity;
pub mod agent_observer;
mod agent_push;
pub mod attention;
pub mod audit_log;
pub mod auto_update;
mod claude_usage;
pub mod codex_usage;
mod connection;
pub mod device_store;
mod direct_push;
mod directory_list;
mod git_changes;
pub mod herdr_cli;
pub mod herdr_client;
pub mod herdr_socket;
pub mod host_files;
pub mod host_heartbeat;
pub mod host_path_mentions;
mod http;
pub mod install;
pub mod management;
mod pane_guard;
mod pane_screen;
mod pane_terminal;
mod push_plugin;
pub use push_plugin::{PluginUpdate, PluginUpdater};
#[rustfmt::skip] // 由 codegen 產生。
pub mod protocol;
pub mod remote_management;
pub mod running_service;
pub mod service_log;
pub mod shutdown;
mod slash_list;
pub mod state_directory;
mod structure_operations;
mod text_format;
pub mod transcript;
pub mod transcript_reads;
mod transcript_session;
pub mod update;
pub mod update_push;
pub mod uploads;
pub mod usage_push;

mod service_main;
mod session_history;

use audit_log::AuditLog;
use device_store::DeviceStore;
use herdr_cli::HerdrCli;
use herdr_client::HerdrClient;
use herdr_socket::HerdrSocket;
pub use http::WEBSOCKET_PATH;
pub use service_main::run_service;

/// 主機服務的設定。位址固定是 127.0.0.1,對外由 `tailscale serve` 提供 HTTPS。
#[derive(Debug, Clone)]
pub struct HostServiceConfig {
    /// 0 表示由系統挑一個空的埠(測試用)。
    pub port: u16,
    pub herdr_socket: HerdrSocket,
    /// Pane 終端機與推播外掛代裝執行的 herdr CLI。
    pub herdr_cli: HerdrCli,
    /// 裝置、配對碼與稽核紀錄所在的資料夾。
    pub state_directory: PathBuf,
    pub heartbeat: Heartbeat,
    /// 對話紀錄所在的家目錄與設定目錄。
    pub transcripts: transcript::TranscriptEnvironment,
    /// 主機的暫存目錄(`TMPDIR`／`TEMP`);主機取檔放行 Agent 提到的暫存檔。
    pub temporary_directory: Option<PathBuf>,
    /// 從 app 管理主機服務(更新、自動更新);None 時 app 收到 host_service_not_installed。
    pub service_management: Option<remote_management::ServiceManagement>,
    /// 常駐的 Agent 觀察者(Send When Done 佇列);None 時不啟動(測試預設)。
    pub observer: Option<agent_observer::ObserverOptions>,
    /// Codex 方案用量執行的 codex。
    pub codex_cli: codex_usage::CodexCli,
}

/// 預設埠(127.0.0.1 上)。
pub const DEFAULT_PORT: u16 = 47_820;

/// WebSocket 心跳：每 `interval` 送一次 ping;`timeout` 內沒收到 app 的任何 frame(含 pong)就斷線。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heartbeat {
    pub interval: Duration,
    pub timeout: Duration,
}

impl Default for Heartbeat {
    /// app 每 15 秒 ping 一次，所以 45 秒沒消息就算斷了。
    fn default() -> Self {
        Heartbeat {
            interval: Duration::from_secs(15),
            timeout: Duration::from_secs(45),
        }
    }
}

impl HostServiceConfig {
    /// 由命令列參數建立設定:`--port <埠>`、`--session <herdr session>`、`--state-dir <資料夾>`;其他參數一律拒絕。
    pub fn from_arguments(arguments: &[String], platform: herdr_socket::Platform, environment: &herdr_socket::SocketEnvironment) -> Result<Self, String> {
        let mut port = DEFAULT_PORT;
        let mut session = None;
        let mut state_directory = None;
        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            let mut value = || remaining.next().ok_or_else(|| format!("{argument} needs a value"));
            match argument.as_str() {
                "--port" => {
                    let text = value()?;
                    port = text.parse().map_err(|_| format!("invalid port: {text}"))?;
                }
                "--session" => session = Some(value()?.clone()),
                "--state-dir" => state_directory = Some(PathBuf::from(value()?)),
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        let herdr_socket =
            HerdrSocket::resolve(platform, session.as_deref(), environment).map_err(|error| format!("cannot resolve herdr socket: {error:?}"))?;
        let state_directory = state_directory
            .or_else(|| state_directory::resolve_state_directory(platform, environment))
            .ok_or("cannot find the home directory; pass --state-dir")?;
        Ok(HostServiceConfig {
            port,
            herdr_socket,
            herdr_cli: HerdrCli::search(session.as_deref()),
            state_directory,
            heartbeat: Heartbeat::default(),
            transcripts: transcript::TranscriptEnvironment::resolve(platform, environment),
            temporary_directory: Some(std::env::temp_dir()),
            service_management: crate::install::HostEnvironment::from_process()
                .ok()
                .and_then(remote_management::for_installed_copy),
            observer: Some(agent_observer::ObserverOptions::default()),
            codex_cli: codex_usage::CodexCli::search(),
        })
    }
}

/// 執行中的主機服務;drop 時停止接受新連線。
pub struct RunningHostService {
    local_addr: SocketAddr,
    accept_loop: tokio::task::JoinHandle<()>,
    /// 定期刪除超過 7 天的上傳檔;找不到家目錄時沒有。
    upload_cleanup: Option<tokio::task::JoinHandle<()>>,
    /// Agent 觀察者的常駐 task。
    observer: Option<tokio::task::JoinHandle<()>>,
    /// 主機心跳的常駐 task。
    host_heartbeat: Option<tokio::task::JoinHandle<()>>,
    /// 方案用量推播的常駐 task(raychiutw/purrdr.host#98)。
    usage_push: Option<tokio::task::JoinHandle<()>>,
    /// 直接送推播的 lease;停止時收回，外掛才會立刻接回。
    lease: Option<direct_push::Lease>,
    /// 與服務共用 serial 鎖的外掛更新器(自動更新排程用)。
    plugin_updater: PluginUpdater,
}

impl RunningHostService {
    /// 實際綁定的位址。
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// 與服務自己處理安裝／移除共用同一把鎖的外掛更新器。
    pub fn plugin_updater(&self) -> PluginUpdater {
        self.plugin_updater.clone()
    }

    /// 一直執行到接受連線的迴圈結束(正式執行用)。
    pub async fn wait(mut self) {
        let _ = (&mut self.accept_loop).await;
    }
}

impl Drop for RunningHostService {
    fn drop(&mut self) {
        self.accept_loop.abort();
        if let Some(upload_cleanup) = &self.upload_cleanup {
            upload_cleanup.abort();
        }
        if let Some(observer) = &self.observer {
            observer.abort();
        }
        if let Some(host_heartbeat) = &self.host_heartbeat {
            host_heartbeat.abort();
        }
        if let Some(usage_push) = &self.usage_push {
            usage_push.abort();
        }
        if let Some(lease) = &self.lease {
            lease.remove();
        }
    }
}

/// 綁定 127.0.0.1 並開始接受連線。
pub async fn start(config: HostServiceConfig) -> std::io::Result<RunningHostService> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", config.port)).await?;
    let local_addr = listener.local_addr()?;
    let log = service_log::ServiceLog::under(&config.state_directory);
    log.record(&format!("purrdr-host-service {} listening on http://{local_addr}", env!("CARGO_PKG_VERSION")));
    // 新資料夾在暫存目錄底下;舊的家目錄資料夾只清理。
    let uploads = config.temporary_directory.as_deref().map(uploads::UploadDirectory::under_temporary_directory);
    let upload_cleanup = {
        let directories: Vec<_> = uploads
            .iter()
            .cloned()
            .chain(config.transcripts.home.as_deref().map(uploads::UploadDirectory::legacy_under_home))
            .collect();
        (!directories.is_empty()).then(|| uploads::spawn_cleanup(directories, log.clone(), std::time::SystemTime::now))
    };
    let socket_path = config.herdr_socket.path().to_owned();
    let herdr = HerdrClient::new(config.herdr_socket);
    let push = push_plugin::PushPlugin::new(config.herdr_cli.clone(), config.transcripts.home.clone()).with_observer(config.observer.is_some());
    // 主機服務直接送 blocked／done 與維護 lease(raychiutw/purrdr.host#48):需要觀察者與家目錄。
    let plugin_updater = push.updater();
    let direct = config
        .observer
        .and(config.transcripts.home.as_deref())
        .map(|home| direct_push::DirectDelivery::new(push.clone(), home, &socket_path));
    let lease = direct.as_ref().map(direct_push::DirectDelivery::lease);
    // 主機心跳與睡眠回報(raychiutw/purrdr.host#99):同樣需要觀察者(正式執行)與家目錄。
    let host_heartbeat = config.observer.and(config.transcripts.home.clone()).map(|home| {
        tokio::spawn(host_heartbeat::run(
            home,
            update_push::HttpPushTransport::default(),
            host_heartbeat::Schedule::default(),
            host_heartbeat::system_sleep_signals(),
        ))
    });
    let observer = config.observer.map(|options| {
        let observer = agent_observer::AgentObserver::load(&config.state_directory);
        let task = agent_observer::spawn(herdr.clone(), observer.clone(), options, config.transcripts.home.clone(), direct);
        (observer, task)
    });
    // 方案用量推播(raychiutw/purrdr.host#98):跟觀察者一起開關(測試關掉觀察者時不讀真的用量)，需要家目錄。
    let usage_push = config
        .observer
        .and(config.transcripts.home.clone())
        .map(|home| usage_push::spawn(home, config.codex_cli.clone(), usage_push::UsagePushOptions::default()));
    let router = http::router(http::ServiceContext {
        herdr,
        observer: observer.as_ref().map(|(observer, _)| observer.clone()),
        push,
        herdr_cli: config.herdr_cli,
        codex_cli: config.codex_cli,
        devices: DeviceStore::new(&config.state_directory),
        audit: AuditLog::new(&config.state_directory),
        log: log.clone(),
        heartbeat: config.heartbeat,
        uploads,
        files: host_files::FileAccessEnvironment {
            home: config.transcripts.home.clone(),
            temporary_directory: config.temporary_directory,
            state_directory: config.state_directory.clone(),
        },
        transcripts: config.transcripts,
        service_management: config.service_management,
    });
    let accept_log = log;
    let accept_loop = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            accept_log.record(&format!("host service stopped: {error}"));
        }
    });
    Ok(RunningHostService {
        local_addr,
        accept_loop,
        upload_cleanup,
        observer: observer.map(|(_, task)| task),
        usage_push,
        lease,
        plugin_updater,
        host_heartbeat,
    })
}
