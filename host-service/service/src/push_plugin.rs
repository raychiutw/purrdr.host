//! 推播外掛代裝(raychiutw/purrdr.ios#261、ADR-0004):以本機使用者身分執行 `herdr plugin list|install|uninstall`,
//! 寫入或移除 `~/.purrdr/push-<裝置>/`。檔案位置與安裝紀錄和 app 的 SSH 路徑(`PushHost`)相同，兩條路徑可互相移除。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::direct_push::PluginPresence;
use crate::herdr_cli::HerdrCli;
use crate::protocol::{PushInstallResult, PushPairing, PushRemoveResult, PushStatusResult, PushUpdateMutedResult, ServerMessage};
use crate::update_push::{HttpPushTransport, Pairing};

/// 主機外掛(`host-plugin/herdr-plugin.toml`)的 id。
pub const PLUGIN_ID: &str = "purrdr.push";
/// 安裝來源固定，app 不能指定(與 app 的 `PushSettings.pluginSource` 相同)。
pub const PLUGIN_SOURCE: &str = "raychiutw/purrdr.host/host-plugin";

/// app 的 `ReversibleHostWrite` 以 `journal.json` 判斷已安裝;沒有改使用者檔案，所以是空的紀錄。
const EMPTY_JOURNAL: &str = r#"{"entries":[],"version":1}"#;
/// 「外掛是 Purrdr 裝的」的紀錄資料夾(裝置 id 不能是 `plugin`)。
const PLUGIN_MARKER: &str = "push-plugin";
/// `herdr plugin install` 要從 GitHub 下載。
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ENDPOINT_LENGTH: usize = 2048;
/// 測試通知的主機名稱上限(字數)。
const MAX_HOST_NAME_LENGTH: usize = 64;
/// 後台：不認得這組配對(raychiutw/purrdr.server#19)。
const PAIRING_NOT_FOUND: u16 = 404;
/// 後台：這組配對已撤銷。
const PAIRING_REVOKED: u16 = 410;

/// 更新推播外掛的結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginUpdate {
    /// 沒裝外掛:不代裝,什麼都不做。
    NotInstalled,
    /// `herdr plugin update` 成功。
    Updated,
}

/// 自動更新排程用的外掛更新器。與服務本身共用同一把 serial 鎖(由 `PushPlugin::updater` 取得),
/// 所以自動更新不會和 app 觸發的安裝／移除同時跑。
#[derive(Debug, Clone)]
pub struct PluginUpdater(PushPlugin);

impl PluginUpdater {
    /// 獨立的更新器(自己一把鎖)。只給沒有服務可共用的情況(測試);正式執行一律用服務的那份。
    pub fn standalone(herdr: HerdrCli) -> Self {
        PluginUpdater(PushPlugin::new(herdr, None))
    }

    /// 外掛有裝才更新(`herdr plugin update purrdr.push --yes`),失敗回 herdr 的訊息。
    pub async fn update(&self) -> Result<PluginUpdate, String> {
        self.0.update_plugin().await.map_err(|failure| failure.to_string())
    }
}

/// Windows:不開 console 視窗(背景執行檔沒有 console)。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 這台主機對某支裝置的推播狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PushState {
    pub plugin_installed: bool,
    pub paired: bool,
    pub other_devices: i64,
    /// 主機服務會直接送 blocked／done(0.12.0 起);此時 `push.install` 不裝外掛。
    pub direct_push: bool,
}

impl PushState {
    pub fn status_result(request_id: String, state: PushState) -> ServerMessage {
        ServerMessage::PushStatusResult(PushStatusResult {
            request_id,
            plugin_installed: state.plugin_installed,
            paired: state.paired,
            other_devices: state.other_devices,
            direct_push: Some(state.direct_push),
        })
    }

    pub fn paired_result(request_id: String, state: PushState) -> ServerMessage {
        ServerMessage::PushInstallResult(PushInstallResult {
            request_id,
            plugin_installed: state.plugin_installed,
            paired: state.paired,
            other_devices: state.other_devices,
            direct_push: Some(state.direct_push),
        })
    }

    pub fn muted_result(request_id: String, state: PushState) -> ServerMessage {
        ServerMessage::PushUpdateMutedResult(PushUpdateMutedResult {
            request_id,
            plugin_installed: state.plugin_installed,
            paired: state.paired,
            other_devices: state.other_devices,
            direct_push: None,
        })
    }

    pub fn unpaired_result(request_id: String, state: PushState) -> ServerMessage {
        ServerMessage::PushRemoveResult(PushRemoveResult {
            request_id,
            plugin_installed: state.plugin_installed,
            paired: state.paired,
            other_devices: state.other_devices,
            direct_push: Some(state.direct_push),
        })
    }
}

#[derive(Debug)]
pub(crate) enum PushFailure {
    /// 裝置 id 或配對內容不合格。
    InvalidRequest(&'static str),
    /// 找不到或執行不了 herdr。
    HerdrUnavailable(String),
    /// herdr 指令失敗;帶 herdr 的輸出。
    Herdr(String),
    /// 這支裝置在這台主機沒有推播配對。
    NotPaired,
    /// 測試通知送不出去：後台的錯誤碼、`http <狀態碼>` 或連線錯誤(不含密鑰)。
    SendFailed(String),
    /// 家目錄或檔案操作失敗。
    Internal(String),
}

impl std::fmt::Display for PushFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushFailure::InvalidRequest(message) => formatter.write_str(message),
            PushFailure::HerdrUnavailable(message) | PushFailure::Herdr(message) | PushFailure::SendFailed(message) | PushFailure::Internal(message) => {
                formatter.write_str(message)
            }
            PushFailure::NotPaired => formatter.write_str("not paired"),
        }
    }
}

impl From<std::io::Error> for PushFailure {
    fn from(error: std::io::Error) -> Self {
        PushFailure::Internal(error.to_string())
    }
}

/// 推播外掛與這台主機的推播配對。同一時間只處理一個請求(多支裝置同時開關時，外掛只裝或移除一次)。
#[derive(Debug, Clone)]
pub(crate) struct PushPlugin {
    herdr: HerdrCli,
    home: Option<PathBuf>,
    serial: Arc<Mutex<()>>,
    /// Agent 觀察者有在執行(能直接送 blocked／done)。還要外掛不是舊版才算真的能直接送。
    observer_running: bool,
}

impl PushPlugin {
    pub fn new(herdr: HerdrCli, home: Option<PathBuf>) -> Self {
        PushPlugin {
            herdr,
            home,
            serial: Arc::new(Mutex::new(())),
            observer_running: false,
        }
    }

    /// 標記 Agent 觀察者有在執行。主機服務直接送還需要家目錄(寫 lease 的地方),
    /// 所以沒有家目錄時一律當作不能直接送:`push.install` 照常裝外掛，不會變成誰都不送。
    pub fn with_observer(mut self, running: bool) -> Self {
        self.observer_running = running && self.home.is_some();
        self
    }

    /// 取得共用這份 serial 鎖的更新器。
    pub fn updater(&self) -> PluginUpdater {
        PluginUpdater(self.clone())
    }

    pub async fn status(&self, device_id: &str) -> Result<PushState, PushFailure> {
        validate_device_id(device_id)?;
        let _serial = self.serial.lock().await;
        self.state(device_id).await
    }

    /// 以這把 key 寫入(或覆寫)這支裝置的配對，外掛還沒安裝就安裝;安裝失敗時還原成寫入前的樣子。
    pub async fn pair(&self, device_id: &str, pairing: &PushPairing) -> Result<PushState, PushFailure> {
        validate_device_id(device_id)?;
        let contents = pairing_file(&pairing.endpoint, &pairing.key, pairing.muted.clone().unwrap_or_default())?;
        let _serial = self.serial.lock().await;
        let purrdr = self.purrdr_directory()?;
        let device_directory = purrdr.join(format!("push-{device_id}"));
        let presence = self.plugin_presence().await?;
        let before = FeatureBackup::take(&purrdr, &device_directory)?;
        write_feature(&device_directory, &[("pairing", contents.as_str())])?;
        // 主機服務會直接送(0.12.0 起)就不裝外掛:同一則通知不會由兩邊各送一次，Windows 也不用 git。
        if presence == PluginPresence::Absent && !self.observer_running {
            let marker = purrdr.join(PLUGIN_MARKER);
            let marker_existed = marker.exists();
            let installed = match write_feature(&marker, &[("source", &format!("{PLUGIN_SOURCE}\n"))]) {
                Ok(()) => {
                    self.run(self.herdr.program_arguments(), &["plugin", "install", PLUGIN_SOURCE, "--yes"], INSTALL_TIMEOUT)
                        .await
                }
                Err(error) => Err(error.into()),
            };
            if let Err(failure) = installed {
                if !marker_existed {
                    let _ = remove_directory(&marker);
                }
                before.restore();
                return Err(failure);
            }
        }
        self.state(device_id).await
    }

    /// 外掛有裝才更新到最新版;沒裝不代裝。
    pub async fn update_plugin(&self) -> Result<PluginUpdate, PushFailure> {
        let _serial = self.serial.lock().await;
        if !self.plugin_presence().await?.installed() {
            return Ok(PluginUpdate::NotInstalled);
        }
        // 與安裝一樣要下載,用較長的逾時;plugin update 對所有 session 生效,不帶 `--session`。
        self.run(self.herdr.program_arguments(), &["plugin", "update", PLUGIN_ID, "--yes"], INSTALL_TIMEOUT)
            .await?;
        Ok(PluginUpdate::Updated)
    }

    /// 只改寫配對檔的靜音清單(保留 key 與 endpoint),不呼叫 herdr。
    pub async fn update_muted(&self, device_id: &str, muted: Vec<String>) -> Result<PushState, PushFailure> {
        validate_device_id(device_id)?;
        validate_muted(&muted)?;
        let _serial = self.serial.lock().await;
        let purrdr = self.purrdr_directory()?;
        let file = purrdr.join(format!("push-{device_id}")).join("pairing");
        let current = match std::fs::read_to_string(&file) {
            Ok(current) => current,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(PushFailure::NotPaired),
            Err(error) => return Err(error.into()),
        };
        let Some(credentials) = PairingCredentials::parse(&current) else {
            return Err(PushFailure::NotPaired);
        };
        write_private_file(&file, &pairing_file(&credentials.endpoint, &credentials.key, muted)?)?;
        Ok(PushState {
            plugin_installed: true,
            paired: true,
            other_devices: other_pairings(&purrdr, device_id),
            direct_push: false,
        })
    }

    /// 送一則測試通知給這支裝置(#60):對它的配對 endpoint 送 `{key, event: test, host_name?}`,不經過主機外掛與 herdr。
    /// 後台回 404／410 是沒有配對(410 時刪掉這組配對，期間重新配對的不刪);其他失敗帶後台的錯誤碼。
    pub async fn send_test(&self, device_id: &str, host_name: Option<&str>) -> Result<(), PushFailure> {
        validate_device_id(device_id)?;
        validate_host_name(host_name)?;
        let pairing = Pairing::read(&self.purrdr_directory()?.join(format!("push-{device_id}"))).ok_or(PushFailure::NotPaired)?;
        let mut body = serde_json::json!({"key": pairing.key, "event": "test"});
        if let Some(host_name) = host_name {
            body["host_name"] = Value::from(host_name);
        }
        let (endpoint, body) = (pairing.endpoint.clone(), body.to_string());
        let reply = tokio::task::spawn_blocking(move || HttpPushTransport::default().post_json_with_reply(&endpoint, &body))
            .await
            .map_err(|error| PushFailure::Internal(error.to_string()))?;
        match reply {
            Ok((status, _)) if (200..300).contains(&status) => Ok(()),
            Ok((PAIRING_NOT_FOUND, _)) => Err(PushFailure::NotPaired),
            Ok((PAIRING_REVOKED, _)) => {
                // 與 push.install／push.remove 互斥：不刪到剛重新寫入的配對。
                let _serial = self.serial.lock().await;
                pairing.remove_if_unchanged();
                Err(PushFailure::NotPaired)
            }
            Ok((status, reply)) => Err(PushFailure::SendFailed(backend_error_code(&reply).unwrap_or_else(|| format!("http {status}")))),
            Err(error) => Err(PushFailure::SendFailed(error)),
        }
    }

    /// 移除這支裝置的配對;沒有其他裝置的配對、且外掛是 Purrdr 裝的才移除外掛。
    pub async fn unpair(&self, device_id: &str) -> Result<PushState, PushFailure> {
        validate_device_id(device_id)?;
        let _serial = self.serial.lock().await;
        let purrdr = self.purrdr_directory()?;
        remove_directory(&purrdr.join(format!("push-{device_id}")))?;
        let marker = purrdr.join(PLUGIN_MARKER);
        if other_pairings(&purrdr, device_id) == 0 && marker.join("journal.json").is_file() {
            if self.plugin_presence().await?.installed() {
                self.run(self.herdr.program_arguments(), &["plugin", "uninstall", PLUGIN_ID], COMMAND_TIMEOUT)
                    .await?;
            }
            remove_directory(&marker)?;
        }
        self.state(device_id).await
    }

    async fn state(&self, device_id: &str) -> Result<PushState, PushFailure> {
        let purrdr = self.purrdr_directory()?;
        let presence = self.plugin_presence().await?;
        Ok(PushState {
            plugin_installed: presence.installed(),
            direct_push: self.observer_running && presence.host_service_may_send(),
            paired: purrdr.join(format!("push-{device_id}")).join("pairing").is_file(),
            other_devices: other_pairings(&purrdr, device_id),
        })
    }

    fn purrdr_directory(&self) -> Result<PathBuf, PushFailure> {
        self.home
            .as_ref()
            .map(|home| home.join(".purrdr"))
            .ok_or_else(|| PushFailure::Internal("cannot find the home directory".into()))
    }

    /// 經執行中的 herdr 伺服器查詢，所以帶 `--session`。
    pub async fn presence(&self) -> Result<PluginPresence, PushFailure> {
        self.plugin_presence().await
    }

    async fn plugin_presence(&self) -> Result<PluginPresence, PushFailure> {
        let stdout = self
            .run(
                &self.herdr.leading_arguments(),
                &["plugin", "list", "--plugin", PLUGIN_ID, "--json"],
                COMMAND_TIMEOUT,
            )
            .await?;
        Ok(PluginPresence::from_list(&stdout))
    }

    /// 執行 herdr,回傳 stdout;非 0 結束時以 stderr(沒有就 stdout)為失敗訊息。
    async fn run(&self, leading: &[String], subcommand: &[&str], limit: Duration) -> Result<String, PushFailure> {
        let mut command = Command::new(self.herdr.program());
        command
            .args(leading)
            .args(subcommand)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let child = command
            .spawn()
            .map_err(|error| PushFailure::HerdrUnavailable(format!("cannot run herdr: {error}")))?;
        let output = tokio::time::timeout(limit, child.wait_with_output())
            .await
            .map_err(|_| PushFailure::Herdr(format!("herdr {} timed out", subcommand.join(" "))))??;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(PushFailure::Herdr(if stderr.is_empty() { stdout.trim().to_owned() } else { stderr }))
    }
}

/// 會拼進主機路徑：只接受 1～64 個小寫英數與 `-`,且不能是 `plugin`(與 app 的 `PushHost.isValidDeviceID` 相同)。
fn validate_device_id(device_id: &str) -> Result<(), PushFailure> {
    let valid = !device_id.is_empty()
        && device_id.len() <= 64
        && device_id != "plugin"
        && device_id.bytes().all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(PushFailure::InvalidRequest("invalid push device id"))
    }
}

/// 配對檔裡的後台位址與密鑰。
pub(crate) struct PairingCredentials {
    pub endpoint: String,
    pub key: String,
}

impl PairingCredentials {
    /// 取 `endpoint=` 與 `key=` 兩行(不檢查格式);缺任何一行時 None。
    pub fn parse(contents: &str) -> Option<PairingCredentials> {
        let value = |name: &str| contents.lines().find_map(|line| line.strip_prefix(name)).map(str::to_owned);
        Some(PairingCredentials {
            endpoint: value("endpoint=")?,
            key: value("key=")?,
        })
    }
}

/// 測試通知標題用的主機名稱：1–64 個字(同後台的上限)、不含控制字元。
fn validate_host_name(host_name: Option<&str>) -> Result<(), PushFailure> {
    let Some(host_name) = host_name else { return Ok(()) };
    let length = host_name.chars().count();
    if (1..=MAX_HOST_NAME_LENGTH).contains(&length) && !host_name.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(PushFailure::InvalidRequest("host_name must be 1–64 characters without control characters"))
    }
}

/// 後台錯誤本文 `{"error": {"code": …}}` 的錯誤碼;只收小寫英數與底線(不把任意內容轉給 app)。
fn backend_error_code(reply: &str) -> Option<String> {
    let value: Value = serde_json::from_str(reply).ok()?;
    let code = value["error"]["code"].as_str()?;
    let is_plain = !code.is_empty() && code.len() <= 64 && code.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    is_plain.then(|| code.to_owned())
}

/// 配對檔(app 的 `PushPairingFile`):一行一個 `名稱=值`,外掛的 sh 與 PowerShell 不需要 JSON 解析器。
fn pairing_file(endpoint: &str, key: &str, mut muted: Vec<String>) -> Result<String, PushFailure> {
    let endpoint_is_valid = endpoint.starts_with("https://")
        && endpoint.len() <= MAX_ENDPOINT_LENGTH
        && !endpoint.chars().any(|character| character.is_whitespace() || character.is_control());
    if !endpoint_is_valid {
        return Err(PushFailure::InvalidRequest("push endpoint must be an https URL"));
    }
    let key_is_valid = key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !key_is_valid {
        return Err(PushFailure::InvalidRequest("push key must be 64 lowercase hex digits"));
    }
    validate_muted(&muted)?;
    muted.sort();
    muted.dedup();
    let mut contents = format!("purrdr-push 1\nendpoint={endpoint}\nkey={key}\n");
    for session in muted {
        contents.push_str(&format!("muted={session}\n"));
    }
    Ok(contents)
}

fn validate_muted(muted: &[String]) -> Result<(), PushFailure> {
    if muted.iter().any(|session| session.is_empty() || session.chars().any(char::is_control)) {
        return Err(PushFailure::InvalidRequest("muted sessions must be single lines"));
    }
    Ok(())
}

/// 其他裝置的配對數(`push-*/pairing`,不含自己)。
fn other_pairings(purrdr: &Path, device_id: &str) -> i64 {
    let own = format!("push-{device_id}");
    let Ok(entries) = std::fs::read_dir(purrdr) else { return 0 };
    let count = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("push-") && name != own && entry.path().join("pairing").is_file()
        })
        .count();
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// 寫入配對前的樣子：安裝外掛失敗時還原(原本有的檔案寫回，原本沒有的資料夾移除)。
struct FeatureBackup {
    purrdr: PathBuf,
    purrdr_existed: bool,
    directory: PathBuf,
    /// `None`:資料夾原本不存在。
    files: Option<Vec<(PathBuf, Vec<u8>)>>,
}

impl FeatureBackup {
    fn take(purrdr: &Path, directory: &Path) -> std::io::Result<FeatureBackup> {
        let files = if directory.is_dir() {
            let mut files = Vec::new();
            for entry in std::fs::read_dir(directory)? {
                let path = entry?.path();
                if path.is_file() {
                    let contents = std::fs::read(&path)?;
                    files.push((path, contents));
                }
            }
            Some(files)
        } else {
            None
        };
        Ok(FeatureBackup {
            purrdr: purrdr.to_owned(),
            purrdr_existed: purrdr.exists(),
            directory: directory.to_owned(),
            files,
        })
    }

    fn restore(self) {
        match self.files {
            None => {
                let _ = remove_directory(&self.directory);
                if !self.purrdr_existed {
                    let _ = std::fs::remove_dir(&self.purrdr);
                }
            }
            Some(files) => {
                for name in ["pairing", "journal.json"] {
                    let path = self.directory.join(name);
                    if !files.iter().any(|(original, _)| *original == path) {
                        let _ = std::fs::remove_file(&path);
                    }
                }
                for (path, contents) in files {
                    let _ = std::fs::write(path, contents);
                }
            }
        }
    }
}

/// 寫一個 `~/.purrdr/<功能>/`:安裝紀錄加上自己的檔案，只有本人可讀。
fn write_feature(directory: &Path, files: &[(&str, &str)]) -> std::io::Result<()> {
    create_private_directory(directory)?;
    write_private_file(&directory.join("journal.json"), EMPTY_JOURNAL)?;
    for (name, contents) in files {
        write_private_file(&directory.join(name), contents)?;
    }
    Ok(())
}

fn remove_directory(directory: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(unix)]
fn create_private_directory(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(directory)
}

#[cfg(not(unix))]
fn create_private_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)
}

/// 先寫暫存檔再改名，外掛不會讀到寫一半的配對。
fn write_private_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let temporary = path.with_extension("partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use std::io::Write;
        let mut file = options.open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn herdr() -> HerdrCli {
        HerdrCli::at_path(PathBuf::from("herdr"), Vec::new())
    }

    #[test]
    fn direct_push_needs_both_the_observer_and_a_known_home_directory() {
        let home = Some(PathBuf::from("/home/demo"));
        assert!(PushPlugin::new(herdr(), home.clone()).with_observer(true).observer_running);
        assert!(!PushPlugin::new(herdr(), home).with_observer(false).observer_running);
        assert!(
            !PushPlugin::new(herdr(), None).with_observer(true).observer_running,
            "沒有家目錄就沒有 lease，不能直接送"
        );
    }

    #[tokio::test]
    async fn the_updater_waits_for_the_services_own_serial_lock() {
        // 不存在的執行檔:鎖放開後 spawn 立刻失敗,不會碰到真的 herdr。
        let missing = HerdrCli::at_path(PathBuf::from("/nonexistent/purrdr-test-herdr"), Vec::new());
        let plugin = PushPlugin::new(missing, None);
        let updater = plugin.updater();
        let held = plugin.serial.lock().await;

        let blocked = tokio::time::timeout(Duration::from_millis(200), updater.update()).await;
        assert!(blocked.is_err(), "服務正在處理安裝／移除時，自動更新要等");

        drop(held);
        let finished = tokio::time::timeout(Duration::from_secs(5), updater.update()).await;
        assert!(finished.is_ok(), "鎖放開後自動更新就能跑");
    }
}
