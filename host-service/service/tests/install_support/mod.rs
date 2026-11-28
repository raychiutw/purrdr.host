//! 安裝測試共用：模擬 tailscale、launchctl、systemctl、reg 的假主機，以及暫存家目錄。

#![allow(dead_code)]

pub mod push_backend;
pub mod release;

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use purrdr_host_service::install::settings::{RequestedSettings, Source, Sourced};
use purrdr_host_service::install::system::{CommandOutput, HostSystem};
use purrdr_host_service::install::{HostEnvironment, InstallOptions, InstallPlatform};
use serde_json::{Value, json};

static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

/// 假主機服務的埠取自系統不會自動分配的範圍(Linux 臨時埠從 32768、macOS／Windows 從 49152 起),
/// 停掉再重啟之間不會被別的測試的 `bind(0)` 或對外連線拿走。
const RESERVED_PORTS: std::ops::Range<u32> = 20_000..32_000;
static NEXT_RESERVED_PORT: AtomicU32 = AtomicU32::new(0);

/// 這個測試程序專用、目前沒人在聽的埠。
pub fn reserved_port() -> u16 {
    let span = RESERVED_PORTS.end - RESERVED_PORTS.start;
    // 同時跑的其他測試程序從不同位置開始。
    let process_offset = std::process::id().wrapping_mul(97) % span;
    loop {
        let index = (process_offset + NEXT_RESERVED_PORT.fetch_add(1, Ordering::SeqCst)) % span;
        let port = u16::try_from(RESERVED_PORTS.start + index).unwrap();
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

pub const FQDN: &str = "studio.tail1234.ts.net";
pub const LAUNCH_AGENT_LABEL: &str = "com.raychiu.purrdr.host-service";
pub const SYSTEMD_UNIT: &str = "purrdr-host-service.service";
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "PurrdrHostService";

/// 測試結束時刪除的暫存資料夾。
pub struct TempDirectory(pub PathBuf);

impl TempDirectory {
    pub fn new(label: &str) -> TempDirectory {
        let path = std::env::temp_dir().join(format!(
            "purrdr-install-test-{label}-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TempDirectory(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 資料夾內所有檔案與資料夾(相對路徑 → 檔案內容;資料夾為 None)。
pub fn snapshot(root: &Path) -> BTreeMap<String, Option<String>> {
    fn walk(root: &Path, directory: &Path, entries: &mut BTreeMap<String, Option<String>>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                entries.insert(format!("{relative}/"), None);
                walk(root, &path, entries);
            } else if path.file_name().is_some_and(|name| name == "update.lock") {
                // 鎖檔沒有內容;Windows 上被鎖住時也讀不了。
                entries.insert(relative, None);
            } else {
                entries.insert(relative, Some(String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned()));
            }
        }
    }
    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries);
    entries
}

/// 一台測試主機：暫存家目錄、放執行檔的下載資料夾、假系統。
pub struct TestHost {
    pub home: TempDirectory,
    pub downloads: TempDirectory,
    pub platform: InstallPlatform,
}

impl TestHost {
    pub fn new(platform: InstallPlatform) -> TestHost {
        let downloads = TempDirectory::new("downloads");
        for name in executable_names(platform) {
            std::fs::write(downloads.0.join(name), format!("binary {name}")).unwrap();
        }
        TestHost {
            home: TempDirectory::new("home"),
            downloads,
            platform,
        }
    }

    pub fn environment(&self) -> HostEnvironment {
        HostEnvironment {
            platform: self.platform,
            home: self.home.0.clone(),
            xdg_config_home: None,
            herdr_config_file: None,
            app_data: None,
            user_name: Some("demo".into()),
            current_executable: self.downloads.0.join(executable_names(self.platform)[0]),
            shell: None,
        }
    }

    pub fn options(&self, port: u16) -> InstallOptions {
        InstallOptions {
            requested: RequestedSettings {
                port: Some(Sourced {
                    value: port,
                    source: Source::Argument,
                }),
                ..RequestedSettings::default()
            },
            source_directory: self.downloads.0.clone(),
        }
    }

    /// 指定 `--no-path` 的安裝選項。
    pub fn options_without_path(&self, port: u16) -> InstallOptions {
        let mut options = self.options(port);
        options.requested.add_to_path = Some(Sourced {
            value: false,
            source: Source::Argument,
        });
        options
    }

    /// 指定 `--https-port` 的安裝選項。
    pub fn options_on_https_port(&self, port: u16, https_port: u16) -> InstallOptions {
        let mut options = self.options(port);
        options.requested.https_port = Some(Sourced {
            value: https_port,
            source: Source::Argument,
        });
        options
    }

    pub fn service_directory(&self) -> PathBuf {
        self.home.0.join(".purrdr").join("host-service")
    }
}

pub fn executable_names(platform: InstallPlatform) -> Vec<&'static str> {
    match platform {
        InstallPlatform::Windows => vec!["purrdr-host-service.exe", "purrdr-host-service-background.exe"],
        _ => vec!["purrdr-host-service"],
    }
}

/// 假主機上可觀察的系統狀態(不含家目錄的檔案)。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SystemState {
    /// `tailscale serve`:`<主機>:<埠>` → 路徑 → proxy 目標。
    pub serve: BTreeMap<String, BTreeMap<String, String>>,
    /// launchd 已載入的 label。
    pub launchd_loaded: BTreeSet<String>,
    /// systemd user 已啟用的 unit。
    pub systemd_enabled: BTreeSet<String>,
    /// systemd user 執行中的 unit。
    pub systemd_active: BTreeSet<String>,
    /// HKCU Run 的值。
    pub run_values: BTreeMap<String, String>,
    /// 背景執行中的程式(路徑與參數)。
    pub background_processes: Vec<Vec<String>>,
    /// Windows 使用者層級 PATH(`HKCU\Environment` 的 `Path`);None 表示沒有這個值。
    pub user_path: Option<String>,
    /// 新程序啟動時 `running.json` 還留著上一個程序的記載的次數(真的服務會因此記「上次未正常關閉」)。
    pub stale_marker_starts: u32,
}

/// 另一個工具已經用 `tailscale serve` 占用的 HTTPS 埠(例如 Allymote 的 443)。
pub fn occupy_https_port(system: &mut FakeSystem, https_port: u16, target: &str) {
    system
        .state
        .serve
        .insert(format!("{FQDN}:{https_port}"), [("/".to_owned(), target.to_owned())].into());
}

/// 可設定前置條件的假系統。
pub struct FakeSystem {
    pub home: PathBuf,
    /// systemd 讀 user unit 的資料夾。
    pub systemd_user_directory: PathBuf,
    pub state: SystemState,
    /// 找得到 tailscale 的路徑;None 表示沒安裝。
    pub tailscale_program: Option<String>,
    pub backend_state: String,
    pub magic_dns: bool,
    pub https_certificates: bool,
    /// `tailscale serve` 設定時回 Access denied(Linux 未設 operator)。
    pub serve_denied: bool,
    /// launchctl／systemctl／reg 是否失敗(例如沒有 systemd user session)。
    pub autostart_fails: bool,
    /// 模擬 launchd 的 bootout 是非同步的：bootout 後 `print` 還要再回報這麼多次「job 仍在」，期間 `bootstrap` 回 error 5。
    pub bootout_lag: u32,
    /// 還沒卸載完的 job 與剩餘的 `print` 次數。
    pub unloading: Option<(String, u32)>,
    pub commands: Vec<String>,
    /// 每次 taskkill 執行當下,看門狗的停止標記(`stop-requested`)是否已存在。
    pub stop_marker_at_taskkill: Vec<bool>,
    /// 等行程結束後才刪的資料夾(fake 會立刻刪)。
    pub deferred_removals: Vec<PathBuf>,
    /// 假主機服務啟動時在 127.0.0.1 聽的埠;None 表示不聽(安裝測試的執行檔不是假主機服務)。
    pub service_port: Option<u16>,
    /// 執行中的假主機服務。
    pub service_listener: Option<TcpListener>,
}

/// 正式版的 Apple Team ID(更新只接受這個 Team 的 Developer ID 簽章)。
pub const TEAM_ID: &str = "8Z6WVFJ574";

/// 假主機服務執行檔的內容：`--version` 回報 `version`;`traits` 含 `broken`(啟動失敗)、`unsigned`(macOS 簽章不符)、
/// `stale`(會聽埠，但執行中的服務回報的不是這個版本，例如重啟沒有真的換掉舊程序)。
pub fn fake_host_service(version: &str, traits: &str) -> String {
    format!("fake-host-service {version} {traits}")
}

impl FakeSystem {
    pub fn ready(home: &Path) -> FakeSystem {
        FakeSystem {
            home: home.to_path_buf(),
            systemd_user_directory: home.join(".config/systemd/user"),
            state: SystemState::default(),
            tailscale_program: Some("tailscale".into()),
            backend_state: "Running".into(),
            magic_dns: true,
            https_certificates: true,
            serve_denied: false,
            autostart_fails: false,
            bootout_lag: 0,
            unloading: None,
            commands: Vec::new(),
            stop_marker_at_taskkill: Vec::new(),
            deferred_removals: Vec::new(),
            service_port: None,
            service_listener: None,
        }
    }

    /// 啟動 `executable`:是能正常執行的假主機服務時開始聽 `service_port`,並像真的服務一樣寫下 `running.json`。
    fn start_service(&mut self, executable: &Path) {
        let contents = std::fs::read_to_string(executable).unwrap_or_default();
        if !contents.starts_with("fake-host-service ") || contents.contains("broken") {
            return;
        }
        let version = if contents.contains("stale") {
            "0.0.1"
        } else {
            contents.split_whitespace().nth(1).unwrap()
        };
        let started_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        let marker = json!({"version": version, "port": self.service_port, "pid": 4242, "started_at_ms": started_at});
        let marker_file = self.home.join(".purrdr/host-service/running.json");
        if marker_file.exists() {
            self.state.stale_marker_starts += 1;
        }
        std::fs::write(marker_file, marker.to_string()).unwrap();
        if let Some(port) = self.service_port {
            self.service_listener = Some(TcpListener::bind(("127.0.0.1", port)).expect("假主機服務的埠被占用"));
        }
    }

    fn stop_service(&mut self) {
        self.service_listener = None;
    }

    fn installed_executable(&self) -> PathBuf {
        let name = if self.home.join(".purrdr/host-service/bin/purrdr-host-service.exe").exists() {
            "purrdr-host-service.exe"
        } else {
            "purrdr-host-service"
        };
        self.home.join(".purrdr/host-service/bin").join(name)
    }

    /// 執行假主機服務的執行檔(`--version`)。
    fn host_service_executable(&mut self, program: &Path, arguments: &[String]) -> CommandOutput {
        let contents = std::fs::read_to_string(program).unwrap();
        let mut words = contents.split_whitespace();
        assert_eq!(words.next(), Some("fake-host-service"), "不是假主機服務：{}", program.display());
        let version = words.next().unwrap();
        match arguments {
            [flag] if flag == "--version" && !contents.contains("broken") => ok(&format!("purrdr-host-service {version}\n")),
            [flag] if flag == "--version" => failed("dyld: missing symbol\n"),
            other => panic!("假主機服務不認得：{other:?}"),
        }
    }

    fn codesign(&mut self, arguments: &[String]) -> CommandOutput {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match arguments.as_slice() {
            ["--verify", "--strict", requirement, file] => {
                assert!(
                    requirement.contains(&format!("certificate leaf[subject.OU] = \"{TEAM_ID}\"")),
                    "一律要求正式版的 Team ID:{requirement}"
                );
                if std::fs::read_to_string(file).unwrap_or_default().contains("unsigned") {
                    failed(&format!("{file}: code object is not signed at all\n"))
                } else {
                    ok("")
                }
            }
            other => panic!("假 codesign 不認得：{other:?}"),
        }
    }

    fn tailscale(&mut self, arguments: &[String]) -> CommandOutput {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match arguments.as_slice() {
            ["version"] => ok("1.90.0\n"),
            ["status", "--json"] => {
                let mut status = json!({
                    "BackendState": self.backend_state,
                    "Self": {"DNSName": format!("{FQDN}.")},
                    "CurrentTailnet": {"Name": "demo@example.com", "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": self.magic_dns},
                });
                if self.https_certificates {
                    status["CertDomains"] = json!([FQDN]);
                }
                ok(&status.to_string())
            }
            ["serve", "status", "--json"] => {
                if self.state.serve.is_empty() {
                    return ok("{}\n");
                }
                let mut tcp = serde_json::Map::new();
                let mut web = serde_json::Map::new();
                for (host_port, handlers) in &self.state.serve {
                    let port = host_port.rsplit(':').next().unwrap();
                    tcp.insert(port.into(), json!({"HTTPS": true}));
                    let handlers: serde_json::Map<String, Value> = handlers.iter().map(|(path, proxy)| (path.clone(), json!({"Proxy": proxy}))).collect();
                    web.insert(host_port.clone(), json!({"Handlers": handlers}));
                }
                ok(&json!({"TCP": tcp, "Web": web}).to_string())
            }
            ["serve", "--bg", https, target] if https.starts_with("--https=") => {
                if self.serve_denied {
                    return failed("Access denied: serve config denied\nUse 'sudo tailscale serve ...' or set operator\n");
                }
                let port = https.trim_start_matches("--https=");
                self.state
                    .serve
                    .entry(format!("{FQDN}:{port}"))
                    .or_default()
                    .insert("/".into(), (*target).into());
                ok("")
            }
            ["serve", https, "off"] if https.starts_with("--https=") => {
                let port = https.trim_start_matches("--https=");
                self.state.serve.remove(&format!("{FQDN}:{port}"));
                ok("")
            }
            other => panic!("假 tailscale 不認得：{other:?}"),
        }
    }

    fn launchctl(&mut self, arguments: &[String]) -> CommandOutput {
        if self.autostart_fails {
            return failed("Bootstrap failed: 5: Input/output error\n");
        }
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match arguments.as_slice() {
            ["bootstrap", "gui/501", plist] => {
                assert!(Path::new(plist).is_file(), "bootstrap 前 plist 要存在：{plist}");
                let label = Path::new(plist).file_stem().unwrap().to_string_lossy().into_owned();
                if self.unloading.as_ref().is_some_and(|(_, remaining)| *remaining > 0) {
                    return failed("Bootstrap failed: 5: Input/output error\n");
                }
                self.state.launchd_loaded.insert(label);
                self.start_service(&self.installed_executable());
                ok("")
            }
            ["bootout", target] => {
                let label = target.trim_start_matches("gui/501/");
                self.stop_service();
                if self.state.launchd_loaded.remove(label) {
                    if self.bootout_lag > 0 {
                        self.unloading = Some((label.to_owned(), self.bootout_lag));
                    }
                    ok("")
                } else {
                    failed("Boot-out failed: 3: No such process\n")
                }
            }
            ["kickstart", "-k", target] => {
                let label = target.trim_start_matches("gui/501/");
                if !self.state.launchd_loaded.contains(label) {
                    return failed("Could not find service\n");
                }
                self.stop_service();
                self.start_service(&self.installed_executable());
                ok("")
            }
            ["print", target] => {
                let label = target.trim_start_matches("gui/501/");
                if let Some((_, remaining)) = self.unloading.as_mut().filter(|(unloading, remaining)| unloading == label && *remaining > 0) {
                    *remaining -= 1;
                    return ok("state = running\n");
                }
                if self.state.launchd_loaded.contains(label) {
                    ok("state = running\n")
                } else {
                    failed("Could not find service\n")
                }
            }
            other => panic!("假 launchctl 不認得：{other:?}"),
        }
    }

    fn systemctl(&mut self, arguments: &[String]) -> CommandOutput {
        if self.autostart_fails {
            return failed("Failed to connect to bus: No medium found\n");
        }
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let wants = self.systemd_user_directory.join("default.target.wants");
        match arguments.as_slice() {
            ["--user", "daemon-reload"] => ok(""),
            ["--user", "enable", "--now", unit] => {
                assert!(self.systemd_user_directory.join(unit).is_file(), "enable 前 unit 檔要存在");
                std::fs::create_dir_all(&wants).unwrap();
                std::fs::write(wants.join(unit), "symlink").unwrap();
                self.state.systemd_enabled.insert((*unit).into());
                self.state.systemd_active.insert((*unit).into());
                self.start_service(&self.installed_executable());
                ok("")
            }
            ["--user", "disable", "--now", unit] => {
                let _ = std::fs::remove_file(wants.join(unit));
                self.state.systemd_enabled.remove(*unit);
                self.state.systemd_active.remove(*unit);
                self.stop_service();
                ok("")
            }
            ["--user", "restart", unit] => {
                if !self.state.systemd_enabled.contains(*unit) {
                    return failed("Unit not found\n");
                }
                self.stop_service();
                // SIGTERM 讓真的服務走正常關閉並清掉自己的 running.json。
                let _ = std::fs::remove_file(self.home.join(".purrdr/host-service/running.json"));
                self.start_service(&self.installed_executable());
                self.state.systemd_active.insert((*unit).into());
                ok("")
            }
            ["--user", "is-enabled", unit] => {
                if self.state.systemd_enabled.contains(*unit) {
                    ok("enabled\n")
                } else {
                    failed("disabled\n")
                }
            }
            other => panic!("假 systemctl 不認得：{other:?}"),
        }
    }

    fn reg(&mut self, arguments: &[String]) -> CommandOutput {
        if self.autostart_fails {
            return failed("ERROR: Access is denied.\n");
        }
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match arguments.as_slice() {
            ["add", RUN_KEY, "/v", name, "/t", "REG_SZ", "/d", data, "/f"] => {
                self.state.run_values.insert((*name).into(), (*data).into());
                ok("The operation completed successfully.\n")
            }
            ["delete", RUN_KEY, "/v", name, "/f"] => {
                if self.state.run_values.remove(*name).is_some() {
                    ok("")
                } else {
                    failed("ERROR: The system was unable to find the specified registry key or value.\n")
                }
            }
            ["query", RUN_KEY, "/v", name] => {
                if self.state.run_values.contains_key(*name) {
                    ok("")
                } else {
                    failed("ERROR: unable to find\n")
                }
            }
            other => panic!("假 reg 不認得：{other:?}"),
        }
    }

    fn taskkill(&mut self, arguments: &[String]) -> CommandOutput {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        match arguments.as_slice() {
            ["/F", "/IM", image, "/FI", filter] => {
                assert_eq!(*filter, "USERNAME eq demo");
                self.stop_marker_at_taskkill
                    .push(self.home.join(".purrdr/host-service/stop-requested").exists());
                self.stop_service();
                let before = self.state.background_processes.len();
                self.state
                    .background_processes
                    .retain(|process| !Path::new(&process[0]).file_name().is_some_and(|name| name == *image));
                if self.state.background_processes.len() < before {
                    ok("SUCCESS\n")
                } else {
                    failed("ERROR: not found\n")
                }
            }
            other => panic!("假 taskkill 不認得：{other:?}"),
        }
    }
}

fn ok(stdout: &str) -> CommandOutput {
    CommandOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

fn failed(stderr: &str) -> CommandOutput {
    CommandOutput {
        success: false,
        stdout: String::new(),
        stderr: stderr.into(),
    }
}

impl HostSystem for FakeSystem {
    fn run(&mut self, program: &str, arguments: &[String]) -> std::io::Result<CommandOutput> {
        self.commands.push(format!("{program} {}", arguments.join(" ")));
        if program == "id" && arguments == ["-u"] {
            return Ok(ok("501\n"));
        }
        if program == "launchctl" {
            return Ok(self.launchctl(arguments));
        }
        if program == "systemctl" {
            return Ok(self.systemctl(arguments));
        }
        if program == "reg" {
            return Ok(self.reg(arguments));
        }
        if program == "taskkill" {
            return Ok(self.taskkill(arguments));
        }
        if program == "codesign" {
            return Ok(self.codesign(arguments));
        }
        if self.tailscale_program.as_deref() == Some(program) {
            return Ok(self.tailscale(arguments));
        }
        // 只有暫存家目錄裡的執行檔是假主機服務(真機上可能有真的 tailscale 等程式)。
        if Path::new(program).starts_with(&self.home) && Path::new(program).is_file() {
            return Ok(self.host_service_executable(Path::new(program), arguments));
        }
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{program}: not found")))
    }

    fn run_attached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<bool> {
        Ok(self.run(program, arguments)?.success)
    }

    fn spawn_detached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<()> {
        self.commands.push(format!("spawn {program} {}", arguments.join(" ")));
        if self.autostart_fails {
            return Err(std::io::Error::other("cannot start"));
        }
        assert!(Path::new(program).is_file(), "背景啟動的執行檔要存在：{program}");
        let mut process = vec![program.to_owned()];
        process.extend(arguments.iter().cloned());
        self.state.background_processes.push(process);
        self.start_service(Path::new(program));
        Ok(())
    }

    fn user_path(&mut self) -> std::io::Result<Option<String>> {
        Ok(self.state.user_path.clone())
    }

    fn set_user_path(&mut self, value: Option<&str>) -> std::io::Result<()> {
        self.commands.push(format!("set user PATH {value:?}"));
        self.state.user_path = value.map(str::to_owned);
        Ok(())
    }

    fn remove_directory_after_exit(&mut self, directory: &Path, empty_directories: &[PathBuf]) -> std::io::Result<()> {
        self.deferred_removals.push(directory.to_path_buf());
        std::fs::remove_dir_all(directory)?;
        for empty in empty_directories {
            let _ = std::fs::remove_dir(empty);
        }
        Ok(())
    }
}

/// 詢問介面的假回答：依序回答每個提示，並把畫面上的文字(說明、提示與回答)記成逐字稿。
pub struct ScriptedPrompter {
    answers: std::collections::VecDeque<String>,
    pub transcript: String,
    pub prompts: Vec<String>,
}

impl ScriptedPrompter {
    pub fn new(answers: &[&str]) -> ScriptedPrompter {
        ScriptedPrompter {
            answers: answers.iter().map(|answer| (*answer).to_owned()).collect(),
            transcript: String::new(),
            prompts: Vec::new(),
        }
    }

    /// 還沒用掉的回答。
    pub fn unused(&self) -> Vec<String> {
        self.answers.iter().cloned().collect()
    }
}

impl purrdr_host_service::install::interactive::Prompter for ScriptedPrompter {
    fn say(&mut self, text: &str) {
        self.transcript.push_str(text);
        self.transcript.push('\n');
    }

    fn read_line(&mut self, prompt: &str) -> Option<String> {
        self.prompts.push(prompt.to_owned());
        self.transcript.push_str(prompt);
        let answer = self.answers.pop_front()?;
        self.transcript.push_str(&answer);
        self.transcript.push('\n');
        Some(answer)
    }
}
