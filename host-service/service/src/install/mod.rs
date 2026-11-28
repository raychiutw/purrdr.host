//! 主機服務的安裝、自動啟動與解除安裝(#62、ADR-0010)。
//!
//! 檔案都放在 `~/.purrdr/host-service/`(`~/.purrdr/` 與其他功能共用);解除安裝依安裝紀錄完整還原。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod autostart;
mod command;
mod command_alias;
mod herdr_shortcut;
pub mod interactive;
mod location;
pub mod non_interactive;
pub mod settings;
pub mod system;
mod tailscale;
mod user_path;

pub use command::{report_invalid_arguments, run_command};
pub use command_alias::CommandAlias;
pub use location::{HelperLauncher, InstalledService};
use settings::yes_no;
use system::HostSystem;
use tailscale::Tailscale;

use crate::herdr_client::HerdrClient;
use crate::herdr_socket::{HerdrSocket, Platform, SocketEnvironment};

/// 安裝目標的作業系統。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPlatform {
    MacOs,
    Linux,
    Windows,
}

impl InstallPlatform {
    /// 這支程式執行所在的平台。
    pub fn current() -> Self {
        if cfg!(windows) {
            InstallPlatform::Windows
        } else if cfg!(target_os = "macos") {
            InstallPlatform::MacOs
        } else {
            InstallPlatform::Linux
        }
    }
}

/// `tailscale serve` 在主機服務 HTTPS 埠上的設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeStatus {
    /// 轉到主機服務(proxy 目標)。
    Configured(String),
    NotConfigured,
    /// 轉到別的地方。
    Other(String),
    /// 查不到(例如 Tailscale 沒裝)。
    Unavailable(String),
}

/// herdr 伺服器的狀態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrStatus {
    Running { version: String },
    NotRunning(String),
}

/// `status` 指令的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStatus {
    pub installed: bool,
    pub autostart_registered: bool,
    /// 程序消失後怎麼被帶回來(Windows 看門狗);由系統重啟的平台或沒有登記時 None。
    pub auto_restart: Option<String>,
    /// 127.0.0.1 上的埠有主機服務在聽。
    pub listening: bool,
    pub port: u16,
    /// `tailscale serve` 的 HTTPS 埠。
    pub https_port: u16,
    pub https_address: Option<String>,
    pub serve: ServeStatus,
    pub herdr: HerdrStatus,
    /// 安裝紀錄的 `auto_update`;沒有安裝時 None。
    pub auto_update: Option<bool>,
    /// 主機服務的 log 資料夾(`host-service.log`、`update.log` 都在這裡)。
    pub log_directory: PathBuf,
    /// `running.json` 記載的程序已不存在(服務沒有正常關閉就消失了)時的上次執行。
    pub vanished_run: Option<crate::running_service::RunningService>,
}

impl std::fmt::Display for HostStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "host service: {}", if self.installed { "installed" } else { "not installed" })?;
        writeln!(formatter, "  starts at login: {}", yes_no(self.autostart_registered))?;
        if let Some(note) = &self.auto_restart {
            writeln!(formatter, "  auto restart: {note}")?;
        }
        if let Some(auto_update) = self.auto_update {
            writeln!(formatter, "  auto update: {}", settings::on_off(auto_update))?;
        }
        writeln!(formatter, "  logs: {}", self.log_directory.display())?;
        writeln!(formatter, "  listening on 127.0.0.1:{}: {}", self.port, yes_no(self.listening))?;
        if let Some(run) = &self.vanished_run {
            writeln!(
                formatter,
                "  last run: process is gone, not shut down cleanly ({})",
                crate::shutdown::describe_run(run)
            )?;
        }
        if let Some(address) = &self.https_address {
            writeln!(formatter, "  HTTPS address: {address}")?;
        }
        match &self.serve {
            ServeStatus::Configured(target) => writeln!(formatter, "tailscale serve: HTTPS {} -> {target}", self.https_port)?,
            ServeStatus::NotConfigured => writeln!(formatter, "tailscale serve: HTTPS {} not configured", self.https_port)?,
            ServeStatus::Other(handlers) => writeln!(formatter, "tailscale serve: HTTPS {} serves something else ({handlers})", self.https_port)?,
            ServeStatus::Unavailable(message) => writeln!(formatter, "tailscale serve: unknown ({message})")?,
        }
        match &self.herdr {
            HerdrStatus::Running { version } => write!(formatter, "herdr: running ({version})"),
            HerdrStatus::NotRunning(message) => write!(formatter, "herdr: not running ({message})"),
        }
    }
}

/// 安裝流程需要的主機資訊。
#[derive(Debug, Clone)]
pub struct HostEnvironment {
    pub platform: InstallPlatform,
    pub home: PathBuf,
    /// `XDG_CONFIG_HOME`(Linux 的 systemd user unit 位置)。
    pub xdg_config_home: Option<PathBuf>,
    /// `HERDR_CONFIG_PATH`(herdr config.toml 的位置，優先於預設位置)。
    pub herdr_config_file: Option<PathBuf>,
    /// `APPDATA`(Windows 的 herdr config 資料夾在它底下)。
    pub app_data: Option<PathBuf>,
    /// 登入帳號(Windows 結束背景程式時只結束自己的)。
    pub user_name: Option<String>,
    /// 正在執行的執行檔。
    pub current_executable: PathBuf,
    /// 使用者的 shell(`SHELL`,macOS／Linux 決定把 PATH 寫進哪個設定檔)。
    pub shell: Option<String>,
}

impl HostEnvironment {
    /// 由環境變數建立。
    pub fn resolve(platform: InstallPlatform, variable: impl Fn(&str) -> Option<String>, current_executable: PathBuf) -> Result<Self, InstallError> {
        let non_empty = |name: &str| variable(name).filter(|value| !value.is_empty());
        let home_variable = if platform == InstallPlatform::Windows { "USERPROFILE" } else { "HOME" };
        let home = non_empty(home_variable).ok_or_else(|| InstallError::HostEnvironment(format!("{home_variable} is not set")))?;
        Ok(HostEnvironment {
            platform,
            home: PathBuf::from(home),
            xdg_config_home: non_empty("XDG_CONFIG_HOME").map(PathBuf::from),
            herdr_config_file: non_empty("HERDR_CONFIG_PATH").map(PathBuf::from),
            app_data: non_empty("APPDATA").map(PathBuf::from),
            user_name: non_empty(if platform == InstallPlatform::Windows { "USERNAME" } else { "USER" }),
            current_executable,
            shell: non_empty("SHELL"),
        })
    }

    /// 這個行程的主機資訊。
    pub fn from_process() -> Result<Self, InstallError> {
        let current_executable = std::env::current_exe().map_err(|error| InstallError::HostEnvironment(format!("cannot find this program: {error}")))?;
        Self::resolve(InstallPlatform::current(), |name| std::env::var(name).ok(), current_executable)
    }
}

/// 安裝相關的子命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallCommand {
    Install {
        /// `--port`;沒指定時依 `PURRDR_PORT`、上次安裝、預設值。
        port: Option<u16>,
        herdr_session: Option<String>,
        /// `tailscale serve` 的 HTTPS 埠;沒指定時依環境變數、上次安裝，或 443。
        https_port: Option<u16>,
        assume_yes: bool,
        /// `--json`:結果以單一 JSON 物件輸出到 stdout，隱含不詢問(但不算同意憑證公開)。
        json: bool,
        /// `--auto-update`(`Some(true)`)／`--no-auto-update`(`Some(false)`)。
        auto_update: Option<bool>,
        /// `--path`(`Some(true)`)／`--no-path`(`Some(false)`)。
        add_to_path: Option<bool>,
    },
    Uninstall,
    Status,
    /// `update [--check] [--yes]`:更新到最新版(raychiutw/purrdr.host#6)。
    Update {
        check_only: bool,
        assume_yes: bool,
    },
    /// `version`:已安裝與最新的版本。
    Version,
    /// `auto-update on|off`。
    AutoUpdate {
        enabled: bool,
    },
}

impl InstallCommand {
    /// 第一個參數是 `install`／`uninstall`／`status`／`update`／`version` 時解析;其他交給主機服務本身。
    pub fn parse(arguments: &[String]) -> Option<Result<Self, String>> {
        let (name, options) = arguments.split_first()?;
        let command = match name.as_str() {
            "install" => Self::parse_install(options),
            "uninstall" | "status" | "version" => match options.first() {
                Some(extra) => Err(format!("unknown argument: {extra}")),
                None if name == "status" => Ok(InstallCommand::Status),
                None if name == "version" => Ok(InstallCommand::Version),
                None => Ok(InstallCommand::Uninstall),
            },
            "update" => Self::parse_update(options),
            "auto-update" => match options {
                [switch] if switch == "on" || switch == "off" => Ok(InstallCommand::AutoUpdate { enabled: switch == "on" }),
                _ => Err("usage: purrdr auto-update on|off".to_owned()),
            },
            _ => return None,
        };
        Some(command)
    }

    fn parse_update(options: &[String]) -> Result<Self, String> {
        let (mut check_only, mut assume_yes) = (false, false);
        for option in options {
            match option.as_str() {
                "--check" => check_only = true,
                "--yes" => assume_yes = true,
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(InstallCommand::Update { check_only, assume_yes })
    }

    fn parse_install(options: &[String]) -> Result<Self, String> {
        let (mut port, mut herdr_session, mut https_port, mut assume_yes, mut json) = (None, None, None, false, false);
        let (mut auto_update, mut add_to_path): (Option<bool>, Option<bool>) = (None, None);
        // 成對開關互相矛盾時報錯，不靜默以後者為準。
        let pair = |slot: &mut Option<bool>, value: bool, on: &str, off: &str| match *slot {
            Some(earlier) if earlier != value => Err(format!("conflicting options: {on} and {off}")),
            _ => {
                *slot = Some(value);
                Ok(())
            }
        };
        let mut remaining = options.iter();
        while let Some(option) = remaining.next() {
            let mut value = || remaining.next().ok_or_else(|| format!("{option} needs a value"));
            match option.as_str() {
                "--port" => {
                    let text = value()?;
                    port = Some(text.parse().ok().filter(|port| *port != 0).ok_or_else(|| format!("invalid port: {text}"))?);
                }
                "--https-port" => {
                    let text = value()?;
                    https_port = Some(
                        text.parse()
                            .ok()
                            .filter(|port| *port != 0)
                            .ok_or_else(|| format!("invalid HTTPS port: {text}"))?,
                    );
                }
                "--session" => herdr_session = Some(value()?.clone()),
                "--yes" => assume_yes = true,
                "--json" => json = true,
                "--auto-update" => pair(&mut auto_update, true, "--auto-update", "--no-auto-update")?,
                "--no-auto-update" => pair(&mut auto_update, false, "--auto-update", "--no-auto-update")?,
                "--path" => pair(&mut add_to_path, true, "--path", "--no-path")?,
                "--no-path" => pair(&mut add_to_path, false, "--path", "--no-path")?,
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(InstallCommand::Install {
            port,
            herdr_session,
            https_port,
            assume_yes,
            json,
            auto_update,
            add_to_path,
        })
    }

    /// 同一個指令加上 `--yes`(同意憑證公開後重跑用)。
    pub fn agreeing(&self) -> Self {
        let mut command = self.clone();
        if let InstallCommand::Install { assume_yes, .. } = &mut command {
            *assume_yes = true;
        }
        command
    }
}

/// `install` 的選項。
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// 參數或環境變數明確指定的設定;其餘沿用上次安裝，或用預設值。
    pub requested: settings::RequestedSettings,
    /// 放下載的執行檔的資料夾。
    pub source_directory: PathBuf,
}

/// 建議 HTTPS 埠時跳過的候選埠，與占用它的服務(`tailscale serve` 的 `路徑 -> 目標`)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedHttpsPort {
    pub port: u16,
    pub handlers: String,
}

/// 被跳過的候選埠與占用者,例如 `443 is used by / -> http://127.0.0.1:8765; 8443 is used by …`。
pub fn describe_skipped(skipped: &[SkippedHttpsPort]) -> String {
    skipped
        .iter()
        .map(|skipped| format!("{} is used by {}", skipped.port, skipped.handlers))
        .collect::<Vec<_>>()
        .join("; ")
}

/// HTTPS 埠的建議值：`port` 是第一個可用的候選埠(沒有時 `None`),`skipped` 是它前面被別人占用的候選埠。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsPortSuggestion {
    pub port: Option<u16>,
    pub skipped: Vec<SkippedHttpsPort>,
}

/// 安裝途中要使用者同意的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallQuestion {
    /// 沒指定 `--https-port`,預設的埠已被別的服務占用：改用 `alternative`?
    UseAnotherHttpsPort {
        taken_port: u16,
        handlers: String,
        alternative: u16,
        skipped: Vec<SkippedHttpsPort>,
    },
    /// 主機名稱會進 Certificate Transparency 公開紀錄。
    CertificateTransparency { host_name: String },
}

impl std::fmt::Display for InstallQuestion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallQuestion::UseAnotherHttpsPort {
                taken_port,
                handlers,
                alternative,
                ..
            } => write!(
                formatter,
                "tailscale serve already uses HTTPS {taken_port} for another service ({handlers}). \
                 The installer leaves it unchanged and can serve Purrdr on HTTPS {alternative} instead."
            ),
            InstallQuestion::CertificateTransparency { host_name } => write!(formatter, "{}", certificate_transparency_notice(host_name)),
        }
    }
}

/// 預設或指定的 HTTPS 埠已被別的服務占用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsPortConflict {
    /// 被占用的埠。
    pub port: u16,
    /// 占用它的服務(`tailscale serve` 的 `路徑 -> 目標`)。
    pub handlers: String,
    /// 建議改用的埠前面被別人占用的候選埠(含 `port` 本身)。
    pub skipped: Vec<SkippedHttpsPort>,
}

/// 安裝前的計畫：照目前的設定安裝會是什麼樣子;唯讀，不改系統。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    /// 採用的設定;預設的 HTTPS 埠被占用時 `https_port` 已是建議的替代埠。
    pub settings: settings::InstallSettings,
    pub host_name: String,
    pub https_address: String,
    /// 已經安裝過(重新安裝)。
    pub reinstall: bool,
    /// HTTPS 埠被占用的情形;沒有占用時 `None`。
    pub https_port_conflict: Option<HttpsPortConflict>,
    /// 這次要提醒主機名稱會公開(第一次安裝，或主機名稱和上次不同)。
    pub certificate_notice: bool,
}

/// 說明網頁的安裝參數總表。
pub const INSTALL_OPTIONS_URL: &str = "https://raychiutw.github.io/purrdr/host-service/#install-options";
/// 給 AI agent 看的純文字說明。
pub const LLMS_URL: &str = "https://raychiutw.github.io/purrdr/host-service/llms.txt";

/// 安裝完成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub https_address: String,
    /// 沒能完成、但不影響安裝結果的項目(例如關掉換埠前的舊 HTTPS 埠)。
    pub warnings: Vec<String>,
    /// 產生配對碼的指令(安裝好的執行檔 `pair`)。
    pub pair_command: String,
    /// `bin/` 已加進使用者 PATH(`--no-path` 時 false)。
    pub added_to_path: bool,
    /// 這次安裝最後採用的設定與各自的來源。
    pub settings: settings::InstallSettings,
    /// 在 herdr 裡顯示配對 QR code 的快捷鍵;沒加(按鍵已被使用者占用)時是 None。
    pub herdr_shortcut: Option<String>,
    /// `bin/` 有指令別名 `purrdr`(raychiutw/purrdr.host#53)。
    pub command_alias: bool,
}

impl Installed {
    /// 每一項最後採用的值與來源(沒辦法詢問時印出，讓使用者知道裝了什麼)。
    pub fn settings_summary(&self) -> String {
        self.settings.summary()
    }
}

impl std::fmt::Display for Installed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter, "Installed. The host service starts at login.")?;
        writeln!(formatter, "HTTPS address: {}", self.https_address)?;
        writeln!(formatter, "To pair a device:")?;
        // 別名要等新終端機讀到 PATH 才打得到，沒加 PATH 時只給完整路徑。
        let short_name = self.command_alias && self.added_to_path;
        if short_name {
            writeln!(formatter, "  1. Open a new terminal and run: {} pair", command_alias::NAME)?;
            writeln!(formatter, "     (or run it now: {})", self.pair_command)?;
        } else {
            writeln!(formatter, "  1. On this computer run: {}", self.pair_command)?;
        }
        if let Some(shortcut) = &self.herdr_shortcut {
            writeln!(
                formatter,
                "     In herdr, press {shortcut} to show the QR code (restart herdr or run `herdr server reload-config` first)."
            )?;
        }
        write!(
            formatter,
            "  2. In Purrdr, tap Add host -> HTTPS and scan the QR code (or enter {} and the pairing code).",
            self.https_address
        )?;
        if self.added_to_path {
            let name = if self.command_alias { command_alias::NAME } else { "purrdr-host-service" };
            write!(formatter, "\nAdded to your PATH: open a new terminal to run {name} directly.")?;
        }
        // 之後怎麼改：沒加 PATH(或沒有別名)時用完整路徑，複製貼上才打得到。
        let command = if short_name {
            command_alias::NAME.to_owned()
        } else {
            self.pair_command.strip_suffix(" pair").unwrap_or(&self.pair_command).to_owned()
        };
        write!(
            formatter,
            "\n\nChange later:\n  Automatic updates:  {command} auto-update on|off\n  Other settings:     {command} install --<option> ...  (paired devices are kept)\n  All options:        {INSTALL_OPTIONS_URL}\n  For AI agents:      {LLMS_URL}"
        )?;
        Ok(())
    }
}

/// 重跑安裝的指令：`prefix` 加上這次的選項;`--https-port` 放最後(換埠的建議以它替換)。
/// session 名稱依執行的 shell 加引號(Windows 是 PowerShell,其他是 sh)。
pub fn rerun_install_command(prefix: &str, command: &InstallCommand, platform: InstallPlatform) -> String {
    let InstallCommand::Install {
        port,
        herdr_session,
        https_port,
        assume_yes,
        json,
        auto_update,
        add_to_path,
    } = command
    else {
        return prefix.to_owned();
    };
    let mut parts = vec![prefix.to_owned()];
    if let Some(port) = port {
        parts.push(format!("--port {port}"));
    }
    if let Some(session) = herdr_session {
        parts.push(format!("--session {}", shell_argument(session, platform)));
    }
    if *assume_yes {
        parts.push("--yes".to_owned());
    }
    match auto_update {
        Some(true) => parts.push("--auto-update".to_owned()),
        Some(false) => parts.push("--no-auto-update".to_owned()),
        None => {}
    }
    match add_to_path {
        Some(true) => parts.push("--path".to_owned()),
        Some(false) => parts.push("--no-path".to_owned()),
        None => {}
    }
    if *json {
        parts.push("--json".to_owned());
    }
    if let Some(https_port) = https_port {
        parts.push(format!("{HTTPS_PORT_OPTION} {https_port}"));
    }
    parts.join(" ")
}

const HTTPS_PORT_OPTION: &str = "--https-port";

/// 只有英數與 `._-` 時原樣，否則加單引號(sh 的 `'` 寫成 `'\''`,PowerShell 寫成 `''`)。
fn shell_argument(value: &str, platform: InstallPlatform) -> String {
    if !value.is_empty() && value.chars().all(|character| character.is_ascii_alphanumeric() || "._-".contains(character)) {
        return value.to_owned();
    }
    let escaped_quote = if platform == InstallPlatform::Windows { "''" } else { r"'\''" };
    format!("'{}'", value.replace('\'', escaped_quote))
}

/// 解除安裝完成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uninstalled {
    /// 沒能還原的項目。
    pub warnings: Vec<String>,
}

/// 安裝或解除安裝失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    TailscaleMissing,
    TailscaleNotRunning(String),
    MagicDnsDisabled,
    HttpsCertificatesDisabled,
    /// `tailscale serve --https=<埠>` 失敗。
    ServeFailed {
        https_port: u16,
    },
    /// 使用者看過 Certificate Transparency 的說明後取消。
    Declined,
    /// 沒辦法詢問、也沒有 `--yes`:需要同意(`notice` 是要同意的說明)，什麼都沒改。
    ConsentRequired {
        notice: String,
    },
    /// 這個 HTTPS 埠已經轉到別的服務;`available` 是可以改用的埠。
    ServePortTaken {
        https_port: u16,
        handlers: String,
        available: Option<u16>,
    },
    /// 這個 HTTPS 埠的 `tailscale serve` 設定已不是安裝時設的。
    ServeChanged {
        https_port: u16,
        handlers: String,
    },
    Tailscale(String),
    Autostart(String),
    NotInstalled,
    Io(String),
    /// 命令列參數不合法(`--json` 時也以 JSON 回報)。
    InvalidArguments(String),
    /// `PURRDR_*` 環境變數的值不合法。
    InvalidEnvironment(settings::SettingsError),
    /// 抓不到執行安裝需要的主機資訊(例如家目錄的環境變數沒設)。
    HostEnvironment(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.explain("purrdr-host-service install"))
    }
}

impl InstallError {
    /// 給使用者的說明：發生什麼、為什麼，最後一段 `Next:` 是下一步(`rerun` 是重跑安裝的完整指令)。
    pub fn explain(&self, rerun: &str) -> String {
        const DNS_SETTINGS: &str = "https://login.tailscale.com/admin/dns";
        let then_rerun = format!("then run the install again:\n  {rerun}");
        match self {
            InstallError::TailscaleMissing => format!(
                "Tailscale is not installed (the `tailscale` command was not found). \
                 The host service is reached through `tailscale serve`, so it needs Tailscale.\n\
                 Next: install Tailscale from https://tailscale.com/download, sign in, {then_rerun}"
            ),
            InstallError::TailscaleNotRunning(state) => format!(
                "Tailscale is not connected (state: {state}). The HTTPS address only works while this machine is on your tailnet.\n\
                 Next: run `tailscale up` and sign in, {then_rerun}"
            ),
            InstallError::MagicDnsDisabled => format!(
                "MagicDNS is off for this tailnet. The HTTPS address is this machine's MagicDNS name, so it needs MagicDNS.\n\
                 Next: turn on MagicDNS at {DNS_SETTINGS}, {then_rerun}"
            ),
            InstallError::HttpsCertificatesDisabled => format!(
                "HTTPS Certificates are off for this tailnet. `tailscale serve` needs them to get an HTTPS certificate for this machine.\n\
                 Next: click \"Enable HTTPS\" under HTTPS Certificates at {DNS_SETTINGS}, {then_rerun}"
            ),
            InstallError::ServeFailed { https_port } => format!(
                "`tailscale serve --bg --https={https_port}` failed (see its output above).\n\
                 On Linux, allow your account to change serve settings first: sudo tailscale set --operator=$USER\n\
                 Next: fix the problem above, {then_rerun}"
            ),
            InstallError::Declined => "Install cancelled; nothing was changed.".to_owned(),
            InstallError::ConsentRequired { notice } => format!(
                "{notice}\n\nThis install needs your consent and cannot ask (no terminal, CI, or --json), so nothing was changed.\n\
                 Next: to agree, run the install again with --yes:\n  {rerun}"
            ),
            InstallError::ServePortTaken {
                https_port,
                handlers,
                available,
            } => {
                // 重跑指令若已指定這個(被占用的)埠，換埠時以新埠取代。
                let without_taken_port = rerun.strip_suffix(&format!(" {HTTPS_PORT_OPTION} {https_port}")).unwrap_or(rerun);
                let next = match available {
                    Some(available) => format!("Next: install on HTTPS {available} instead:\n  {without_taken_port} {HTTPS_PORT_OPTION} {available}"),
                    None => format!(
                        "Next: pick a free HTTPS port (`tailscale serve status` lists the ones in use) and run:\n  {without_taken_port} {HTTPS_PORT_OPTION} <port>"
                    ),
                };
                format!(
                    "tailscale serve already uses HTTPS {https_port} for another service:\n  {handlers}\n\
                     The installer does not change existing tailscale serve settings.\n\
                     {next}\n\
                     Or, if you no longer need that service, remove it with `tailscale serve --https={https_port} off` and run:\n  {rerun}"
                )
            }
            InstallError::ServeChanged { https_port, handlers } => {
                format!("tailscale serve HTTPS {https_port} now serves {handlers}, left unchanged")
            }
            InstallError::Tailscale(message) => format!(
                "Tailscale returned an error: {message}\n\
                 Next: check Tailscale with `tailscale status`, {then_rerun}"
            ),
            InstallError::Autostart(message) => format!(
                "Could not set the host service to start at login: {message}\n\
                 Next: fix the problem above, {then_rerun}"
            ),
            InstallError::Io(message) => format!("{message}\nNext: fix the problem above, {then_rerun}"),
            InstallError::NotInstalled => "The Purrdr host service is not installed.".to_owned(),
            InstallError::InvalidArguments(message) => format!("{message}\nNext: see the install options with:\n  {rerun}"),
            InstallError::InvalidEnvironment(error) => error.explain(rerun),
            InstallError::HostEnvironment(message) => format!("{message}\nNext: run the install from a normal user session (with HOME or USERPROFILE set)."),
        }
    }

    /// 機器可讀的錯誤種類(`--json` 的 `error.kind`)。
    pub fn kind(&self) -> &'static str {
        match self {
            InstallError::TailscaleMissing => "tailscale_missing",
            InstallError::TailscaleNotRunning(_) => "tailscale_not_running",
            InstallError::MagicDnsDisabled => "magic_dns_disabled",
            InstallError::HttpsCertificatesDisabled => "https_certificates_disabled",
            InstallError::ServeFailed { .. } => "serve_failed",
            InstallError::Declined => "declined",
            InstallError::ConsentRequired { .. } => "consent_required",
            InstallError::ServePortTaken { .. } => "https_port_taken",
            InstallError::ServeChanged { .. } => "serve_changed",
            InstallError::Tailscale(_) => "tailscale_error",
            InstallError::Autostart(_) => "autostart_failed",
            InstallError::NotInstalled => "not_installed",
            InstallError::Io(_) => "io_error",
            InstallError::InvalidArguments(_) => "invalid_arguments",
            InstallError::InvalidEnvironment(_) => "invalid_environment",
            InstallError::HostEnvironment(_) => "host_environment",
        }
    }

    fn io(error: std::io::Error) -> Self {
        InstallError::Io(error.to_string())
    }
}

/// 主機服務在家目錄裡的檔案位置。
pub(crate) struct ServiceLayout {
    platform: InstallPlatform,
    home: PathBuf,
    xdg_config_home: Option<PathBuf>,
}

impl ServiceLayout {
    pub(crate) fn of(environment: &HostEnvironment) -> Self {
        ServiceLayout {
            platform: environment.platform,
            home: environment.home.clone(),
            xdg_config_home: environment.xdg_config_home.clone(),
        }
    }

    pub(crate) fn directory(&self) -> PathBuf {
        crate::state_directory::default_state_directory(&self.home)
    }

    fn bin_directory(&self) -> PathBuf {
        self.directory().join("bin")
    }

    fn executable_name(&self) -> &'static str {
        match self.platform {
            InstallPlatform::Windows => "purrdr-host-service.exe",
            _ => "purrdr-host-service",
        }
    }

    fn executable(&self) -> PathBuf {
        self.bin_directory().join(self.executable_name())
    }

    /// Windows 自動啟動用的、不開 console 視窗的執行檔。
    fn background_executable_name(&self) -> &'static str {
        "purrdr-host-service-background.exe"
    }

    fn background_executable(&self) -> PathBuf {
        self.bin_directory().join(self.background_executable_name())
    }

    /// 要從下載資料夾複製到 `bin/` 的執行檔。
    fn executable_names(&self) -> Vec<&'static str> {
        match self.platform {
            InstallPlatform::Windows => vec![self.executable_name(), self.background_executable_name()],
            _ => vec![self.executable_name()],
        }
    }

    fn log_file(&self) -> PathBuf {
        self.directory().join("logs").join("host-service.log")
    }

    fn config_file(&self) -> PathBuf {
        self.directory().join("config.json")
    }

    fn record_file(&self) -> PathBuf {
        self.directory().join("install.json")
    }
}

/// 安裝紀錄：解除安裝依它還原。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct InstallRecord {
    port: u16,
    herdr_session: Option<String>,
    /// `tailscale serve` 的 HTTPS 埠(0.2.0 的紀錄沒有這欄，當時固定 443)。
    #[serde(default = "default_https_port")]
    https_port: u16,
    https_address: String,
    /// 是否由安裝流程設定了 `tailscale serve`。
    serve_configured: bool,
    /// 安裝時新建的資料夾(相對家目錄),解除安裝時若是空的就刪除。
    created_directories: Vec<String>,
    /// 背景自動更新(0.5.0 以前的紀錄沒有這欄，視為開啟)。
    #[serde(default = "enabled")]
    auto_update: bool,
    /// 把 `bin/` 加進使用者 PATH(`--no-path` 關閉;0.6.0 以前的紀錄沒有這欄，視為開啟)。
    #[serde(default = "enabled")]
    add_to_path: bool,
    /// 安裝對 PATH 做的變更。
    #[serde(default)]
    path: user_path::PathChange,
    /// 安裝對 herdr config 加的快捷鍵(0.8.0 及以前的紀錄沒有這欄)。
    #[serde(default)]
    herdr_shortcut: herdr_shortcut::ShortcutChange,
}

impl InstallRecord {
    /// 上次安裝時的主機名稱(取自 HTTPS 網址)。
    fn host_name(&self) -> &str {
        let address = self.https_address.strip_prefix("https://").unwrap_or(&self.https_address);
        address.split([':', '/']).next().unwrap_or_default()
    }

    fn settings_snapshot(&self) -> settings::PreviousSettings {
        settings::PreviousSettings {
            port: self.port,
            https_port: self.https_port,
            herdr_session: self.herdr_session.clone(),
            auto_update: self.auto_update,
            add_to_path: self.add_to_path,
        }
    }
}

fn enabled() -> bool {
    true
}

fn default_https_port() -> u16 {
    tailscale::DEFAULT_HTTPS_PORT
}

/// 主機服務讀取的設定(`pair` 以 `https_address` 當預設位址)。
#[derive(Serialize)]
struct ServiceConfig<'a> {
    https_address: &'a str,
}

/// 一個 HTTPS 埠的使用狀況。
enum PortUse {
    Free,
    /// 已轉到主機服務;`in_place` 表示目標就是這次要用的本機埠。
    Ours {
        in_place: bool,
    },
    Taken(String),
}

/// 安裝與解除安裝主機服務。
pub struct Installer<S: HostSystem> {
    environment: HostEnvironment,
    system: S,
    /// 最近一次 `install` 已解析到的設定(安裝失敗時仍可讀，讓呼叫端說明各項設定與來源)。
    attempted: Option<settings::InstallSettings>,
}

impl<S: HostSystem> Installer<S> {
    pub fn new(environment: HostEnvironment, system: S) -> Self {
        Installer {
            environment,
            system,
            attempted: None,
        }
    }

    /// 安裝用的系統(測試檢查假系統狀態)。
    pub fn system(&self) -> &S {
        &self.system
    }

    /// 安裝用的系統(測試調整假系統狀態)。
    pub fn system_mut(&mut self) -> &mut S {
        &mut self.system
    }

    /// 最近一次 `install` 已解析到的設定;還沒解析到(例如讀安裝紀錄就失敗)時 `None`。
    pub fn attempted_settings(&self) -> Option<&settings::InstallSettings> {
        self.attempted.as_ref()
    }

    /// 取回安裝用的系統(測試接著交給更新流程)。
    pub fn into_system(self) -> S {
        self.system
    }

    fn layout(&self) -> ServiceLayout {
        ServiceLayout::of(&self.environment)
    }

    /// 檢查前置條件、複製執行檔、設定 `tailscale serve` 與登入後自動啟動。
    /// `confirm` 回答安裝途中的問題(`InstallQuestion`),回 false 就取消(此時還沒有改任何東西)。
    pub fn install(&mut self, options: &InstallOptions, mut confirm: impl FnMut(&InstallQuestion) -> bool) -> Result<Installed, InstallError> {
        let layout = self.layout();
        let previous = self.read_record(&layout)?;
        let mut settings = settings::InstallSettings::from_options(&options.requested, previous.as_ref().map(InstallRecord::settings_snapshot));
        self.attempted = Some(settings.clone());
        let tailscale = Tailscale::locate(&mut self.system, self.environment.platform)?;
        let host_name = tailscale.host_name(&mut self.system)?;
        let preferred = settings.https_port.value;
        let (https_port, serve_in_place) = match self.check_https_port(&tailscale, preferred, settings.port.value, previous.as_ref())? {
            PortUse::Free => (preferred, false),
            PortUse::Ours { in_place } => (preferred, in_place),
            PortUse::Taken(handlers) => {
                let suggestion = self.suggest_https_port_with(&tailscale, settings.port.value, previous.as_ref())?;
                let available = suggestion.port.filter(|candidate| *candidate != preferred);
                let taken = InstallError::ServePortTaken {
                    https_port: preferred,
                    handlers: handlers.clone(),
                    available,
                };
                // 使用者指定的埠不擅自換掉。
                let Some(alternative) = available.filter(|_| !settings.https_port.is_explicit()) else {
                    return Err(taken);
                };
                let question = InstallQuestion::UseAnotherHttpsPort {
                    taken_port: preferred,
                    handlers,
                    alternative,
                    skipped: suggestion.skipped,
                };
                if !confirm(&question) {
                    return Err(taken);
                }
                settings.https_port = settings::Sourced {
                    value: alternative,
                    source: settings::Source::Suggested,
                };
                self.attempted = Some(settings.clone());
                (alternative, false)
            }
        };
        // 主機名稱和上次安裝相同就不再提醒。
        let host_name_changed = previous.as_ref().is_none_or(|record| record.host_name() != host_name);
        if host_name_changed && !confirm(&InstallQuestion::CertificateTransparency { host_name: host_name.clone() }) {
            return Err(InstallError::Declined);
        }

        let mut record = InstallRecord {
            port: settings.port.value,
            herdr_session: settings.herdr_session.value.clone(),
            https_port,
            https_address: tailscale::https_address(&host_name, https_port),
            serve_configured: previous.as_ref().is_some_and(|record| record.serve_configured),
            created_directories: previous.as_ref().map(|record| record.created_directories.clone()).unwrap_or_default(),
            auto_update: settings.auto_update.value,
            add_to_path: settings.add_to_path.value,
            path: previous.as_ref().map(|record| record.path.clone()).unwrap_or_default(),
            herdr_shortcut: previous.as_ref().map(|record| record.herdr_shortcut.clone()).unwrap_or_default(),
        };
        if previous.is_some() {
            // 先停掉執行中的舊版，執行檔才換得掉(Windows)。
            autostart::unregister(&mut self.system, &layout, self.environment.user_name.as_deref())?;
        }
        let shortcut = match self.apply(&layout, &tailscale, options, serve_in_place, &mut record) {
            Ok(shortcut) => shortcut,
            Err(error) => {
                // 重新安裝失敗時保留已配對的裝置，不還原。
                if previous.is_none() {
                    self.roll_back(&layout, &tailscale, &record);
                }
                return Err(error);
            }
        };
        let mut warnings = Vec::new();
        let command_alias = match command_alias::ensure(&layout) {
            Ok(command_alias::CommandAlias::Created | command_alias::CommandAlias::Present) => true,
            Ok(command_alias::CommandAlias::Taken(file)) => {
                warnings.push(format!("the purrdr command was not added: {} already exists", file.display()));
                false
            }
            Err(error) => {
                warnings.push(format!("the purrdr command was not added: {error}"));
                false
            }
        };
        let herdr_shortcut = match shortcut {
            herdr_shortcut::Outcome::Added => Some(herdr_shortcut::KEY.to_owned()),
            herdr_shortcut::Outcome::Skipped(warning) => {
                warnings.push(warning);
                None
            }
        };
        // 換了 HTTPS 埠：關掉上次安裝設的舊埠。
        if let Some(previous) = previous.filter(|previous| previous.serve_configured && previous.https_port != record.https_port)
            && let Err(error) = tailscale.remove_serve(&mut self.system, previous.https_port, previous.port)
        {
            warnings.push(format!("the previous HTTPS {} was not removed: {error}", previous.https_port));
        }
        Ok(Installed {
            https_address: record.https_address,
            warnings,
            pair_command: format!("{} pair", shell_quoted(&layout.executable(), self.environment.platform)),
            added_to_path: record.add_to_path,
            settings,
            herdr_shortcut,
            command_alias,
        })
    }

    /// 照目前的設定安裝會是什麼樣子(互動安裝的摘要);唯讀，不改系統。
    pub fn plan(&mut self, options: &InstallOptions) -> Result<InstallPlan, InstallError> {
        let layout = self.layout();
        let previous = self.read_record(&layout)?;
        let tailscale = Tailscale::locate(&mut self.system, self.environment.platform)?;
        let host_name = tailscale.host_name(&mut self.system)?;
        let mut settings = settings::InstallSettings::from_options(&options.requested, previous.as_ref().map(InstallRecord::settings_snapshot));
        let preferred = settings.https_port.value;
        let mut https_port_conflict = None;
        if let PortUse::Taken(handlers) = self.check_https_port(&tailscale, preferred, settings.port.value, previous.as_ref())? {
            let suggestion = self.suggest_https_port_with(&tailscale, settings.port.value, previous.as_ref())?;
            let alternative = suggestion
                .port
                .filter(|candidate| *candidate != preferred && !settings.https_port.is_explicit());
            let skipped = match alternative {
                Some(_) => suggestion.skipped,
                None => vec![SkippedHttpsPort {
                    port: preferred,
                    handlers: handlers.clone(),
                }],
            };
            if let Some(alternative) = alternative {
                settings.https_port = settings::Sourced {
                    value: alternative,
                    source: settings::Source::Suggested,
                };
            }
            https_port_conflict = Some(HttpsPortConflict {
                port: preferred,
                handlers,
                skipped,
            });
        }
        Ok(InstallPlan {
            https_address: tailscale::https_address(&host_name, settings.https_port.value),
            certificate_notice: previous.as_ref().is_none_or(|record| record.host_name() != host_name),
            reinstall: previous.is_some(),
            settings,
            host_name,
            https_port_conflict,
        })
    }

    /// `https_port` 被別的服務占用時回占用它的服務(本機埠 `port`);空的或已是自己的回 `None`。唯讀。
    pub fn https_port_taken_by(&mut self, https_port: u16, port: u16) -> Result<Option<String>, InstallError> {
        let layout = self.layout();
        let previous = self.read_record(&layout)?;
        let tailscale = Tailscale::locate(&mut self.system, self.environment.platform)?;
        Ok(match self.check_https_port(&tailscale, https_port, port, previous.as_ref())? {
            PortUse::Taken(handlers) => Some(handlers),
            PortUse::Free | PortUse::Ours { .. } => None,
        })
    }

    /// 沒指定 HTTPS 埠時的建議值(本機埠 `port`);唯讀，不改系統。
    pub fn suggest_https_port(&mut self, port: u16) -> Result<HttpsPortSuggestion, InstallError> {
        let layout = self.layout();
        let previous = self.read_record(&layout)?;
        let tailscale = Tailscale::locate(&mut self.system, self.environment.platform)?;
        self.suggest_https_port_with(&tailscale, port, previous.as_ref())
    }

    fn suggest_https_port_with(&mut self, tailscale: &Tailscale, port: u16, previous: Option<&InstallRecord>) -> Result<HttpsPortSuggestion, InstallError> {
        // 重新安裝：上次的埠仍是自己的就不換。
        if let Some(previous) = previous
            && matches!(
                self.check_https_port(tailscale, previous.https_port, port, Some(previous))?,
                PortUse::Ours { .. }
            )
        {
            return Ok(HttpsPortSuggestion {
                port: Some(previous.https_port),
                skipped: Vec::new(),
            });
        }
        let mut skipped = Vec::new();
        for candidate in tailscale::CANDIDATE_HTTPS_PORTS {
            match self.check_https_port(tailscale, candidate, port, previous)? {
                PortUse::Taken(handlers) => skipped.push(SkippedHttpsPort { port: candidate, handlers }),
                PortUse::Free | PortUse::Ours { .. } => {
                    return Ok(HttpsPortSuggestion {
                        port: Some(candidate),
                        skipped,
                    });
                }
            }
        }
        Ok(HttpsPortSuggestion { port: None, skipped })
    }

    /// HTTPS 埠目前是空的、已是主機服務的，還是被別的服務占用。
    fn check_https_port(&mut self, tailscale: &Tailscale, https_port: u16, port: u16, previous: Option<&InstallRecord>) -> Result<PortUse, InstallError> {
        let handlers = tailscale.https_handlers(&mut self.system, https_port)?;
        let proxy_to = |port: u16| BTreeMap::from([("/".to_owned(), tailscale::serve_target(port))]);
        if handlers.is_empty() {
            return Ok(PortUse::Free);
        }
        if handlers == proxy_to(port) {
            return Ok(PortUse::Ours { in_place: true });
        }
        let ours_from_previous_install =
            previous.is_some_and(|record| record.serve_configured && record.https_port == https_port && handlers == proxy_to(record.port));
        if ours_from_previous_install {
            return Ok(PortUse::Ours { in_place: false });
        }
        Ok(PortUse::Taken(tailscale::describe(&handlers)))
    }

    fn read_record(&self, layout: &ServiceLayout) -> Result<Option<InstallRecord>, InstallError> {
        read_install_record(layout)
    }

    /// 安裝的每一步;`record` 隨時反映已做的變更，失敗時依它還原。
    fn apply(
        &mut self,
        layout: &ServiceLayout,
        tailscale: &Tailscale,
        options: &InstallOptions,
        serve_in_place: bool,
        record: &mut InstallRecord,
    ) -> Result<herdr_shortcut::Outcome, InstallError> {
        for directory in [layout.bin_directory(), layout.log_file().parent().unwrap().to_path_buf()] {
            self.create_directory(&directory, record)?;
        }
        for name in layout.executable_names() {
            let (source, destination) = (options.source_directory.join(name), layout.bin_directory().join(name));
            // 從安裝好的那份再執行 install 時，複製到自己會把檔案清空。
            if !same_file(&source, &destination) {
                std::fs::copy(&source, &destination).map_err(InstallError::io)?;
            }
        }
        write_json(
            &layout.config_file(),
            &ServiceConfig {
                https_address: &record.https_address,
            },
        )?;

        if !serve_in_place {
            record.serve_configured = true;
            tailscale.serve(&mut self.system, record.https_port, record.port)?;
        }

        if record.add_to_path {
            if let Some(profile) = user_path::shell_profile(layout, self.environment.shell.as_deref()) {
                let directory = profile
                    .parent()
                    .ok_or_else(|| InstallError::Io(format!("cannot find the folder of {}", profile.display())))?;
                self.create_directory(directory, record)?;
            }
            user_path::add(&mut self.system, layout, self.environment.shell.as_deref(), &mut record.path)?;
        } else {
            user_path::remove(&mut self.system, layout, &mut record.path)?;
        }

        if let Some(registration) = autostart::registration_file(layout) {
            self.create_directory(registration.parent().unwrap(), record)?;
        }
        for directory in autostart::system_created_directories(layout) {
            if !directory.exists() {
                self.record_created(&directory, record);
            }
        }
        let shortcut = self.add_herdr_shortcut(layout, record);
        write_json(&layout.record_file(), record)?;
        autostart::register(&mut self.system, layout, &service_arguments(record))?;
        Ok(shortcut)
    }

    /// herdr 快捷鍵是附加功能：寫不進 herdr config 時只給警告，不讓安裝失敗。
    fn add_herdr_shortcut(&self, layout: &ServiceLayout, record: &mut InstallRecord) -> herdr_shortcut::Outcome {
        let file = herdr_shortcut::config_file(&self.environment);
        let added = file
            .parent()
            .map_or(Ok(()), |directory| self.create_directory(directory, record))
            .and_then(|()| herdr_shortcut::add(&file, &layout.executable(), self.environment.platform, &mut record.herdr_shortcut));
        added.unwrap_or_else(|error| herdr_shortcut::Outcome::Skipped(format!("the herdr shortcut for pairing was not added to {}: {error}", file.display())))
    }

    /// 安裝失敗時盡力還原;還原本身的錯誤不蓋過原本的錯誤。
    fn roll_back(&mut self, layout: &ServiceLayout, tailscale: &Tailscale, record: &InstallRecord) {
        let _ = autostart::unregister(&mut self.system, layout, self.environment.user_name.as_deref());
        let _ = user_path::remove(&mut self.system, layout, &mut record.path.clone());
        let _ = herdr_shortcut::remove(&mut record.herdr_shortcut.clone());
        if record.serve_configured {
            let _ = tailscale.remove_serve(&mut self.system, record.https_port, record.port);
        }
        let _ = self.remove_files(layout, record);
    }

    /// `auto-update on|off`:改安裝紀錄的 `auto_update`。
    pub fn set_auto_update(&mut self, enabled: bool) -> Result<(), InstallError> {
        let layout = self.layout();
        let mut record = self.read_record(&layout)?.ok_or(InstallError::NotInstalled)?;
        record.auto_update = enabled;
        write_json(&layout.record_file(), &record)
    }

    /// 主機服務、自動啟動、`tailscale serve` 與 herdr 的狀態。
    pub async fn status(&mut self, socket_environment: &SocketEnvironment) -> HostStatus {
        let layout = self.layout();
        let record = self.read_record(&layout).ok().flatten();
        let port = record.as_ref().map_or(crate::DEFAULT_PORT, |record| record.port);
        let https_port = record.as_ref().map_or(tailscale::DEFAULT_HTTPS_PORT, |record| record.https_port);
        let target = tailscale::serve_target(port);
        let serve =
            match Tailscale::locate(&mut self.system, self.environment.platform).and_then(|tailscale| tailscale.https_handlers(&mut self.system, https_port)) {
                Ok(handlers) if handlers.is_empty() => ServeStatus::NotConfigured,
                Ok(handlers) if handlers == BTreeMap::from([("/".to_owned(), target.clone())]) => ServeStatus::Configured(target),
                Ok(handlers) => ServeStatus::Other(tailscale::describe(&handlers)),
                Err(error) => ServeStatus::Unavailable(error.to_string()),
            };
        let session = record.as_ref().and_then(|record| record.herdr_session.clone());
        let listening = is_listening(port).await;
        let autostart_registered = autostart::is_registered(&mut self.system, &layout);
        let auto_update_setting = record.as_ref().map(|record| record.auto_update);
        HostStatus {
            installed: record.is_some(),
            autostart_registered,
            auto_restart: autostart_registered.then(|| autostart::auto_restart_note(&layout)).flatten(),
            listening,
            port,
            https_port,
            https_address: record.map(|record| record.https_address),
            serve,
            herdr: herdr_status(self.environment.platform, session.as_deref(), socket_environment).await,
            auto_update: auto_update_setting,
            log_directory: layout.directory().join("logs"),
            vanished_run: if listening {
                None
            } else {
                crate::shutdown::vanished_previous_run(&layout.directory(), std::process::id(), &crate::shutdown::SystemProcesses)
            },
        }
    }

    /// 移除自動啟動、`tailscale serve` 設定與主機服務的檔案;還原不了的項目列在 `warnings`。
    pub fn uninstall(&mut self) -> Result<Uninstalled, InstallError> {
        let layout = self.layout();
        let mut record = self.read_record(&layout)?.ok_or(InstallError::NotInstalled)?;
        let mut warnings = Vec::new();

        autostart::unregister(&mut self.system, &layout, self.environment.user_name.as_deref())?;
        user_path::remove(&mut self.system, &layout, &mut record.path)?;
        herdr_shortcut::remove(&mut record.herdr_shortcut)?;
        if record.serve_configured {
            let removed = Tailscale::locate(&mut self.system, self.environment.platform)
                .and_then(|tailscale| tailscale.remove_serve(&mut self.system, record.https_port, record.port));
            if let Err(error) = removed {
                warnings.push(format!("tailscale serve was not removed: {error}"));
            }
        }
        self.remove_files(&layout, &record)?;
        Ok(Uninstalled { warnings })
    }

    /// 刪除主機服務的資料夾，再刪除安裝時新建、現在是空的資料夾(由深到淺)。
    fn remove_files(&mut self, layout: &ServiceLayout, record: &InstallRecord) -> Result<(), InstallError> {
        let created: Vec<PathBuf> = record
            .created_directories
            .iter()
            .rev()
            .map(|relative| self.environment.home.join(relative))
            .filter(|directory| !directory.starts_with(layout.directory()))
            .collect();
        let running_from_installed_copy = self.environment.current_executable.starts_with(layout.directory());
        if self.environment.platform == InstallPlatform::Windows && running_from_installed_copy {
            return self.system.remove_directory_after_exit(&layout.directory(), &created).map_err(InstallError::io);
        }
        // taskkill 剛結束的背景程式，Windows 可能還沒放開執行檔，稍等重試。
        let mut attempts_left = 10;
        loop {
            match std::fs::remove_dir_all(layout.directory()) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) if attempts_left > 0 => {
                    attempts_left -= 1;
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                Err(error) => return Err(InstallError::io(error)),
                Ok(()) => break,
            }
        }
        for directory in created {
            let _ = std::fs::remove_dir(directory);
        }
        Ok(())
    }

    /// 建立資料夾，並把新建的每一層記在安裝紀錄。
    fn create_directory(&self, directory: &Path, record: &mut InstallRecord) -> Result<(), InstallError> {
        let mut missing: Vec<&Path> = directory.ancestors().take_while(|ancestor| !ancestor.exists()).collect();
        std::fs::create_dir_all(directory).map_err(InstallError::io)?;
        missing.reverse();
        for created in missing {
            self.record_created(created, record);
        }
        Ok(())
    }

    fn record_created(&self, directory: &Path, record: &mut InstallRecord) {
        if let Ok(relative) = directory.strip_prefix(&self.environment.home) {
            record.created_directories.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// 讀安裝紀錄;沒有安裝時回 None。
fn read_install_record(layout: &ServiceLayout) -> Result<Option<InstallRecord>, InstallError> {
    let text = match std::fs::read_to_string(layout.record_file()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        result => result.map_err(InstallError::io)?,
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| InstallError::Io(format!("{}: {error}", layout.record_file().display())))
}

async fn is_listening(port: u16) -> bool {
    let connect = tokio::net::TcpStream::connect(("127.0.0.1", port));
    matches!(tokio::time::timeout(std::time::Duration::from_secs(2), connect).await, Ok(Ok(_)))
}

async fn herdr_status(platform: InstallPlatform, session: Option<&str>, socket_environment: &SocketEnvironment) -> HerdrStatus {
    let socket_platform = if platform == InstallPlatform::Windows {
        Platform::Windows
    } else {
        Platform::Unix
    };
    let socket = match HerdrSocket::resolve(socket_platform, session, socket_environment) {
        Ok(socket) => socket,
        Err(error) => return HerdrStatus::NotRunning(format!("cannot resolve herdr socket: {error:?}")),
    };
    match HerdrClient::new(socket).request("ping", serde_json::json!({})).await {
        Ok(pong) => HerdrStatus::Running {
            version: pong["version"].as_str().unwrap_or("unknown").to_owned(),
        },
        Err(failure) => HerdrStatus::NotRunning(format!("{failure:?}")),
    }
}

/// 設定 `tailscale serve` 前給使用者看的說明。
fn certificate_transparency_notice(host_name: &str) -> String {
    format!(
        "Tailscale will get an HTTPS certificate for {host_name}. Certificates are recorded in public Certificate Transparency logs, \
         so this name becomes publicly visible. If it reveals something sensitive, rename this machine at \
         https://login.tailscale.com/admin/machines first."
    )
}

/// 自動啟動時傳給主機服務的參數。
fn service_arguments(record: &InstallRecord) -> Vec<String> {
    let mut arguments = vec!["--port".to_owned(), record.port.to_string()];
    if let Some(session) = &record.herdr_session {
        arguments.extend(["--session".to_owned(), session.clone()]);
    }
    arguments
}

/// 終端機可以直接貼上執行的路徑(有空白時加引號;PowerShell 要 `&` 才會執行引號內的路徑)。
pub(crate) fn shell_quoted(path: &Path, platform: InstallPlatform) -> String {
    let text = path.display().to_string();
    match (text.contains(' '), platform) {
        (false, _) => text,
        (true, InstallPlatform::Windows) => format!("& \"{text}\""),
        (true, _) => format!("\"{text}\""),
    }
}

fn same_file(first: &Path, second: &Path) -> bool {
    match (first.canonicalize(), second.canonicalize()) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), InstallError> {
    let text = serde_json::to_string_pretty(value).expect("可序列化");
    std::fs::write(path, text).map_err(InstallError::io)
}
