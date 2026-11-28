//! 安裝流程對作業系統的接縫：執行 tailscale、launchctl、systemctl、reg 等固定程式。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 程式執行完的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 安裝流程用到的系統操作;測試以假系統取代。
pub trait HostSystem {
    /// 執行程式並取得輸出;程式不存在時回 `Err`。
    fn run(&mut self, program: &str, arguments: &[String]) -> std::io::Result<CommandOutput>;
    /// 執行程式並接上終端機(它可能印出網址並等待使用者),回傳是否成功。
    fn run_attached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<bool>;
    /// 在背景啟動程式，不等它結束。
    fn spawn_detached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<()>;
    /// 這個行程結束後刪除資料夾，再刪除仍是空的 `empty_directories`(Windows 不能刪除執行中的執行檔)。
    fn remove_directory_after_exit(&mut self, directory: &Path, empty_directories: &[PathBuf]) -> std::io::Result<()>;
    /// 這個 pid 的程序還在不在(不執行外部指令);停止服務後確認舊程序真的結束用。測試的假系統可覆寫。
    fn process_exists(&mut self, pid: u32) -> bool {
        crate::shutdown::ProcessTable::is_alive(&crate::shutdown::SystemProcesses, pid)
    }
    /// Windows 使用者層級 PATH(`HKCU\Environment` 的 `Path`,不展開 `%…%` 的原始值);沒有這個值時 None。
    fn user_path(&mut self) -> std::io::Result<Option<String>>;
    /// 寫入 Windows 使用者層級 PATH(None 刪除這個值),並通知其他程式環境變數已變更。
    fn set_user_path(&mut self, value: Option<&str>) -> std::io::Result<()>;
}

/// 真正執行程式的系統。這是主機服務裡唯一會執行子行程的地方(`no_command_execution` 測試把關)，
/// 只由本機的 install／uninstall／status 指令使用，WebSocket 連線碰不到。
pub struct ProcessHostSystem;

/// 這個行程的系統操作。`--json` 時 stdout 只放最後那個 JSON 物件，
/// 所以接上終端機執行的子行程(例如 `tailscale serve --bg`)的 stdout 改導到 stderr。
pub struct CliHostSystem {
    pub json: bool,
}

impl HostSystem for CliHostSystem {
    fn run(&mut self, program: &str, arguments: &[String]) -> std::io::Result<CommandOutput> {
        ProcessHostSystem.run(program, arguments)
    }

    fn run_attached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<bool> {
        if !self.json {
            return ProcessHostSystem.run_attached(program, arguments);
        }
        let mut command = Command::new(program);
        command.args(arguments);
        if let Some(stderr) = stderr_stdio() {
            command.stdout(stderr);
        }
        Ok(command.status()?.success())
    }

    fn spawn_detached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<()> {
        ProcessHostSystem.spawn_detached(program, arguments)
    }

    fn remove_directory_after_exit(&mut self, directory: &Path, empty_directories: &[PathBuf]) -> std::io::Result<()> {
        ProcessHostSystem.remove_directory_after_exit(directory, empty_directories)
    }

    fn user_path(&mut self) -> std::io::Result<Option<String>> {
        ProcessHostSystem.user_path()
    }

    fn set_user_path(&mut self, value: Option<&str>) -> std::io::Result<()> {
        ProcessHostSystem.set_user_path(value)
    }
}

/// 複製一份指向本行程 stderr 的 handle，給子行程當 stdout;複製失敗時 `None`(維持原本的 stdout)。
fn stderr_stdio() -> Option<Stdio> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd as _;
        std::io::stderr().as_fd().try_clone_to_owned().ok().map(Stdio::from)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle as _;
        std::io::stderr().as_handle().try_clone_to_owned().ok().map(Stdio::from)
    }
}

/// 背景自動更新：以獨立程序執行 `<執行檔> update --yes`。服務重啟時會結束自己的程序，
/// 所以更新程序要脫離它：Linux 放進另一個 systemd unit(同一個 cgroup 會一起被結束，所以 `systemd_run` 失敗就回報、不改用別的方式),
/// macOS 另開 process group(launchd 只結束 job 的 group),Windows 是不同的執行檔名稱(重啟只結束背景執行檔)。
pub fn spawn_update_helper(executable: &Path, systemd_run: &str) -> std::io::Result<()> {
    let arguments = ["update", "--yes"];
    if cfg!(target_os = "linux") {
        let status = Command::new(systemd_run)
            .args(["--user", "--collect", "--quiet"])
            .arg(executable)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        return if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("{systemd_run} failed ({status})")))
        };
    }
    let mut command = Command::new(executable);
    command.args(arguments).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 隱藏的 console 讓它執行的 taskkill 不會跳出視窗。
        command.creation_flags(windows_flags::CREATE_NO_WINDOW | windows_flags::CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(windows)]
    standard_handles::stop_inheriting();
    let mut child = command.spawn()?;
    // 收掉結束的子程序(服務沒被重啟時)。
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Windows 的 `CreateProcess` 由 Rust 以「繼承所有可繼承的 handle」呼叫(stable 沒有關掉的方法),
/// 所以背景程式會連呼叫端給這個行程的標準 handle(例如 `update --yes 2>&1 | tail` 的管線)一起繼承，
/// 讓管線在 update 結束後仍沒有 EOF(raychiutw/purrdr.host#35)。背景啟動前把自己的標準 handle 設成不可繼承;
/// 之後以 `Stdio::inherit` 執行的程式不受影響(Rust 會另外複製一份可繼承的給它)。
#[cfg(windows)]
mod standard_handles {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

    #[allow(unsafe_code)]
    pub fn stop_inheriting() {
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: GetStdHandle 只讀這個行程的標準 handle(沒有時是 null 或 INVALID_HANDLE_VALUE,不轉移所有權);
            // SetHandleInformation 只清掉該 handle 的繼承旗標，不關閉、不改變它指向的物件。失敗時維持原狀(最差是原本的行為)。
            unsafe {
                let handle = GetStdHandle(which);
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}

#[cfg(windows)]
mod windows_flags {
    pub const DETACHED_PROCESS: u32 = 0x0000_0008;
    pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
}

impl HostSystem for ProcessHostSystem {
    fn run(&mut self, program: &str, arguments: &[String]) -> std::io::Result<CommandOutput> {
        let output = Command::new(program).args(arguments).stdin(Stdio::null()).output()?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn run_attached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<bool> {
        Ok(Command::new(program).args(arguments).status()?.success())
    }

    fn spawn_detached(&mut self, program: &str, arguments: &[String]) -> std::io::Result<()> {
        let mut command = Command::new(program);
        command.args(arguments).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_flags::DETACHED_PROCESS | windows_flags::CREATE_NEW_PROCESS_GROUP);
            standard_handles::stop_inheriting();
        }
        command.spawn().map(drop)
    }

    #[cfg(windows)]
    fn remove_directory_after_exit(&mut self, directory: &Path, empty_directories: &[PathBuf]) -> std::io::Result<()> {
        use std::os::windows::process::CommandExt;
        // 等這個行程結束(約 2 秒)再刪;rmdir 不加 /s 只刪空資料夾。
        let mut script = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", directory.display());
        for empty in empty_directories {
            script.push_str(&format!(" & rmdir \"{}\" 2>nul", empty.display()));
        }
        Command::new("cmd.exe")
            .raw_arg(format!("/d /s /c \"{script}\""))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(windows_flags::CREATE_NO_WINDOW | windows_flags::CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .map(drop)
    }

    #[cfg(windows)]
    fn user_path(&mut self) -> std::io::Result<Option<String>> {
        windows_environment::user_path(windows_environment::USER_ENVIRONMENT_KEY)
    }

    #[cfg(windows)]
    fn set_user_path(&mut self, value: Option<&str>) -> std::io::Result<()> {
        windows_environment::set_user_path(windows_environment::USER_ENVIRONMENT_KEY, value)
    }

    #[cfg(not(windows))]
    fn user_path(&mut self) -> std::io::Result<Option<String>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the user PATH in the registry is Windows only",
        ))
    }

    #[cfg(not(windows))]
    fn set_user_path(&mut self, _value: Option<&str>) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "the user PATH in the registry is Windows only",
        ))
    }

    /// Unix 可以刪除執行中的執行檔，直接刪。
    #[cfg(not(windows))]
    fn remove_directory_after_exit(&mut self, directory: &Path, empty_directories: &[PathBuf]) -> std::io::Result<()> {
        std::fs::remove_dir_all(directory)?;
        for empty in empty_directories {
            let _ = std::fs::remove_dir(empty);
        }
        Ok(())
    }
}

/// Windows 使用者環境變數的登錄(`HKCU\<subkey>` 的 `Path`),以 PowerShell 讀寫;`subkey` 讓測試改用暫存機碼。
#[cfg(windows)]
pub mod windows_environment {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    /// 使用者環境變數所在的機碼(`HKCU\Environment`)。
    pub const USER_ENVIRONMENT_KEY: &str = "Environment";

    /// 讀 `Path` 不展開 `%…%`(寫回時才不會把變數寫死);以 UTF-8 base64 輸出，不受 console 字碼頁影響。
    const READ: &str = r#"$ErrorActionPreference = 'Stop'
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($env:PURRDR_REGISTRY_KEY)
if ($null -eq $key) { exit 3 }
$value = $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
if ($null -eq $value) { exit 3 }
[Console]::Out.Write([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([string]$value)))
"#;

    /// 以 REG_EXPAND_SZ 寫入(或刪除)`Path`;再以 .NET 刪一個不存在的使用者變數，讓它廣播 WM_SETTINGCHANGE。
    /// 依賴 .NET Framework 的 `SetEnvironmentVariable(…, 'User')` 寫完登錄後一律廣播(變數原本不存在也會)。
    const WRITE: &str = r#"$ErrorActionPreference = 'Stop'
$key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($env:PURRDR_REGISTRY_KEY)
if ($env:PURRDR_USER_PATH) {
    $key.SetValue('Path', $env:PURRDR_USER_PATH, [Microsoft.Win32.RegistryValueKind]::ExpandString)
} else {
    $key.DeleteValue('Path', $false)
}
$key.Close()
[Environment]::SetEnvironmentVariable('PURRDR_HOST_SERVICE_ENVIRONMENT_REFRESH', $null, 'User')
"#;

    /// `HKCU\<subkey>` 的 `Path` 原始值;沒有時 None。
    pub fn user_path(subkey: &str) -> std::io::Result<Option<String>> {
        let output = powershell(READ, subkey, None)?;
        if output.status.code() == Some(3) {
            return Ok(None);
        }
        check(&output)?;
        let encoded = String::from_utf8_lossy(&output.stdout);
        let bytes = STANDARD.decode(encoded.trim()).map_err(std::io::Error::other)?;
        String::from_utf8(bytes).map(Some).map_err(std::io::Error::other)
    }

    /// 寫入(None 刪除)`HKCU\<subkey>` 的 `Path`,並通知其他程式環境變數已變更。
    pub fn set_user_path(subkey: &str, value: Option<&str>) -> std::io::Result<()> {
        check(&powershell(WRITE, subkey, Some(value.unwrap_or_default()))?)
    }

    /// 值經環境變數傳入(Unicode,不經命令列引號);腳本以 `-EncodedCommand`(UTF-16LE base64)傳入。
    fn powershell(script: &str, subkey: &str, value: Option<&str>) -> std::io::Result<std::process::Output> {
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
            .arg(STANDARD.encode(utf16))
            .env("PURRDR_REGISTRY_KEY", subkey)
            .stdin(Stdio::null())
            .creation_flags(super::windows_flags::CREATE_NO_WINDOW);
        if let Some(value) = value {
            command.env("PURRDR_USER_PATH", value);
        }
        command.output()
    }

    fn check(output: &std::process::Output) -> std::io::Result<()> {
        if output.status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "powershell failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}
