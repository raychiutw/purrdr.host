//! 登入後自動啟動:macOS LaunchAgent、Linux systemd user unit、Windows HKCU Run 機碼(不需系統管理員)。

use std::path::{Path, PathBuf};

use super::system::HostSystem;
use super::{InstallError, InstallPlatform, ServiceLayout};

/// LaunchAgent 的 label(依 bundle id)。
const LAUNCH_AGENT_LABEL: &str = "com.raychiu.purrdr.host-service";
/// systemd user unit 名稱。
const SYSTEMD_UNIT: &str = "purrdr-host-service.service";
/// 目前使用者登入時執行的程式。
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "PurrdrHostService";

/// 等 launchd 卸載舊 job 的輪詢間隔與次數(合計約 10 秒)。
const UNLOAD_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
const UNLOAD_POLL_LIMIT: u32 = 200;

/// 自動啟動的登記檔(macOS plist、Linux unit);沒有檔案的平台回 None。
pub(super) fn registration_file(layout: &ServiceLayout) -> Option<PathBuf> {
    match layout.platform {
        InstallPlatform::MacOs => Some(layout.home.join("Library/LaunchAgents").join(format!("{LAUNCH_AGENT_LABEL}.plist"))),
        InstallPlatform::Linux => Some(systemd_user_directory(layout).join(SYSTEMD_UNIT)),
        InstallPlatform::Windows => None,
    }
}

/// 登記時由系統建立的資料夾(systemd `enable` 建立的 `.wants`)。
pub(super) fn system_created_directories(layout: &ServiceLayout) -> Vec<PathBuf> {
    match layout.platform {
        InstallPlatform::Linux => vec![systemd_user_directory(layout).join("default.target.wants")],
        _ => Vec::new(),
    }
}

fn systemd_user_directory(layout: &ServiceLayout) -> PathBuf {
    let config_home = layout.xdg_config_home.clone().unwrap_or_else(|| layout.home.join(".config"));
    config_home.join("systemd").join("user")
}

/// 寫好登記檔之後，向系統登記並立即啟動。
pub(super) fn register(system: &mut impl HostSystem, layout: &ServiceLayout, service_arguments: &[String]) -> Result<(), InstallError> {
    let command_line: Vec<String> = std::iter::once(layout.executable().display().to_string())
        .chain(service_arguments.iter().cloned())
        .collect();
    match layout.platform {
        InstallPlatform::MacOs => {
            let plist = registration_file(layout).expect("macOS 有 plist");
            std::fs::write(&plist, launch_agent_plist(&command_line)).map_err(InstallError::io)?;
            let domain = gui_domain(system)?;
            run_checked(system, "launchctl", &["bootstrap".into(), domain, plist.display().to_string()])
        }
        InstallPlatform::Linux => {
            let unit = registration_file(layout).expect("Linux 有 unit");
            std::fs::write(&unit, systemd_unit(&command_line)).map_err(InstallError::io)?;
            run_checked(system, "systemctl", &arguments(&["--user", "daemon-reload"]))?;
            run_checked(system, "systemctl", &arguments(&["--user", "enable", "--now", SYSTEMD_UNIT]))
        }
        InstallPlatform::Windows => {
            // console 程式從 Run 機碼啟動會開視窗，所以登記不開視窗的那支。
            let background = layout.background_executable().display().to_string();
            let run_command = std::iter::once(format!("\"{background}\""))
                .chain(service_arguments.iter().map(|argument| windows_quote(argument)))
                .collect::<Vec<_>>()
                .join(" ");
            let add = [RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &run_command, "/f"];
            run_checked(system, "reg", &std::iter::once("add").chain(add).map(str::to_owned).collect::<Vec<_>>())?;
            system
                .spawn_detached(&background, service_arguments)
                .map_err(|error| InstallError::Autostart(format!("cannot start {background}: {error}")))
        }
    }
}

/// 就地重啟已登記的服務(不刪登記檔、不取消登記):macOS `kickstart -k`、Linux `systemctl --user restart`、
/// Windows 結束背景程序後再啟動一次(Run 值不動)。
pub(super) fn restart(system: &mut impl HostSystem, layout: &ServiceLayout, service_arguments: &[String], user_name: Option<&str>) -> Result<(), InstallError> {
    match layout.platform {
        InstallPlatform::MacOs => {
            let domain = gui_domain(system)?;
            run_checked(
                system,
                "launchctl",
                &["kickstart".into(), "-k".into(), format!("{domain}/{LAUNCH_AGENT_LABEL}")],
            )
        }
        InstallPlatform::Linux => run_checked(system, "systemctl", &arguments(&["--user", "restart", SYSTEMD_UNIT])),
        InstallPlatform::Windows => {
            stop_windows_background(system, layout, user_name);
            let background = layout.background_executable().display().to_string();
            system
                .spawn_detached(&background, service_arguments)
                .map_err(|error| InstallError::Autostart(format!("cannot start {background}: {error}")))
        }
    }
}

/// 只結束這個使用者的背景執行檔。
fn stop_windows_background(system: &mut impl HostSystem, layout: &ServiceLayout, user_name: Option<&str>) {
    let mut taskkill = arguments(&["/F", "/IM", layout.background_executable_name()]);
    if let Some(user_name) = user_name {
        taskkill.extend(["/FI".to_owned(), format!("USERNAME eq {user_name}")]);
    }
    let _ = system.run("taskkill", &taskkill);
}

/// 停止並取消登記，刪除登記檔;沒有登記時不算失敗。
pub(super) fn unregister(system: &mut impl HostSystem, layout: &ServiceLayout, user_name: Option<&str>) -> Result<(), InstallError> {
    match layout.platform {
        InstallPlatform::MacOs => {
            let domain = gui_domain(system)?;
            let job = format!("{domain}/{LAUNCH_AGENT_LABEL}");
            let _ = system.run("launchctl", &["bootout".into(), job.clone()]);
            wait_until_unloaded(system, &job);
        }
        InstallPlatform::Linux => {
            let _ = system.run("systemctl", &arguments(&["--user", "disable", "--now", SYSTEMD_UNIT]));
        }
        InstallPlatform::Windows => {
            stop_windows_background(system, layout, user_name);
            let _ = system.run("reg", &arguments(&["delete", RUN_KEY, "/v", RUN_VALUE, "/f"]));
        }
    }
    if let Some(file) = registration_file(layout) {
        remove_file_if_exists(&file)?;
    }
    if layout.platform == InstallPlatform::Linux {
        let _ = system.run("systemctl", &arguments(&["--user", "daemon-reload"]));
    }
    Ok(())
}

/// `launchctl bootout` 是非同步的:回傳時舊 job 可能還沒卸載完，這時馬上 `bootstrap` 會回 error 5
/// (raychiutw/purrdr.host#51)。輪詢 `launchctl print` 直到 job 消失;等不到就放行，由後面的 bootstrap 回報錯誤。
fn wait_until_unloaded(system: &mut impl HostSystem, job: &str) {
    for attempt in 0..UNLOAD_POLL_LIMIT {
        let still_loaded = system.run("launchctl", &["print".into(), job.to_owned()]).is_ok_and(|output| output.success);
        if !still_loaded {
            return;
        }
        if attempt + 1 < UNLOAD_POLL_LIMIT {
            std::thread::sleep(UNLOAD_POLL_INTERVAL);
        }
    }
}

/// 系統是否登記了自動啟動。
pub(super) fn is_registered(system: &mut impl HostSystem, layout: &ServiceLayout) -> bool {
    let query = match layout.platform {
        InstallPlatform::MacOs => match gui_domain(system) {
            Ok(domain) => ("launchctl", vec!["print".to_owned(), format!("{domain}/{LAUNCH_AGENT_LABEL}")]),
            Err(_) => return false,
        },
        InstallPlatform::Linux => ("systemctl", arguments(&["--user", "is-enabled", SYSTEMD_UNIT])),
        InstallPlatform::Windows => ("reg", arguments(&["query", RUN_KEY, "/v", RUN_VALUE])),
    };
    system.run(query.0, &query.1).is_ok_and(|output| output.success)
}

/// Windows 命令列的一個參數：空的或含空白時加雙引號(session 名稱不會含雙引號)。
fn windows_quote(argument: &str) -> String {
    if argument.is_empty() || argument.contains(char::is_whitespace) {
        format!("\"{argument}\"")
    } else {
        argument.to_owned()
    }
}

fn arguments(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|text| (*text).to_owned()).collect()
}

fn remove_file_if_exists(file: &Path) -> Result<(), InstallError> {
    match std::fs::remove_file(file) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(InstallError::io(error)),
        _ => Ok(()),
    }
}

fn gui_domain(system: &mut impl HostSystem) -> Result<String, InstallError> {
    let output = system.run("id", &["-u".into()]).map_err(|error| InstallError::Autostart(error.to_string()))?;
    Ok(format!("gui/{}", output.stdout.trim()))
}

fn run_checked(system: &mut impl HostSystem, program: &str, arguments: &[String]) -> Result<(), InstallError> {
    let output = system
        .run(program, arguments)
        .map_err(|error| InstallError::Autostart(format!("{program}: {error}")))?;
    if output.success {
        Ok(())
    } else {
        Err(InstallError::Autostart(format!(
            "{program} {} failed: {}",
            arguments.join(" "),
            output.stderr.trim()
        )))
    }
}

fn launch_agent_plist(command_line: &[String]) -> String {
    let program_arguments: String = command_line
        .iter()
        .map(|argument| format!("\n        <string>{}</string>", xml_escape(argument)))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCH_AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>{program_arguments}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
"#
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `ExecStart` 的執行檔與含空白的參數加雙引號;`%` 是 systemd 的 specifier,要寫成 `%%`。
fn systemd_unit(command_line: &[String]) -> String {
    let exec_start = command_line
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let escaped = argument.replace('%', "%%");
            if index == 0 || argument.contains(char::is_whitespace) {
                format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=Purrdr host service\n\n[Service]\nExecStart={exec_start}\nRestart=on-failure\nRestartSec=5\nStandardOutput=null\nStandardError=null\n\n[Install]\nWantedBy=default.target\n"
    )
}
