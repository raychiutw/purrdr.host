//! 執行檔的 install／uninstall／status 子命令(#62)。只跑不改系統的情境：家目錄是暫存資料夾。

mod install_support;

use std::process::{Command, Output};

use install_support::TempDirectory;

/// 以暫存家目錄執行主機服務的執行檔。
fn run(home: &TempDirectory, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_purrdr-host-service"))
        .args(arguments)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("XDG_CONFIG_HOME")
        .env("HERDR_SOCKET_PATH", "purrdr-binary-test-missing-socket")
        .output()
        .unwrap()
}

#[test]
fn status_reports_not_installed_without_changing_home() {
    let home = TempDirectory::new("binary-status");

    let output = run(&home, &["status"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout.contains("host service: not installed"), "{stdout}");
    assert!(stdout.contains("herdr: not running"), "{stdout}");
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

#[test]
fn uninstall_without_install_says_so() {
    let home = TempDirectory::new("binary-uninstall");

    let output = run(&home, &["uninstall"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not installed"), "{output:?}");
}

#[test]
fn invalid_install_arguments_exit_with_usage_error() {
    let home = TempDirectory::new("binary-arguments");

    let output = run(&home, &["install", "--bind", "0.0.0.0"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown argument: --bind"), "{output:?}");
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

#[test]
fn windows_background_executable_is_built_alongside() {
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_purrdr-host-service-background")).is_file());
}

/// 一行安裝腳本與 app 的更新提示都靠它認版本(#198)。
#[test]
fn version_flag_prints_version_without_touching_home() {
    let home = TempDirectory::new("binary-version");

    for flag in ["--version", "-V"] {
        let output = run(&home, &[flag]);

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("purrdr-host-service {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

/// `--help` 列出所有安裝參數、對應的環境變數與說明網址(raychiutw/purrdr.host#85)。
#[test]
fn help_lists_every_install_option_environment_variable_and_the_help_links() {
    let home = TempDirectory::new("binary-help");

    for flag in ["--help", "help"] {
        let output = run(&home, &[flag]);

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{output:?}");
        for expected in [
            "--port",
            "--https-port",
            "--session",
            "--yes",
            "--auto-update",
            "--no-auto-update",
            "--path",
            "--no-path",
            "--json",
            "PURRDR_PORT",
            "PURRDR_HTTPS_PORT",
            "PURRDR_SESSION",
            "PURRDR_AUTO_UPDATE",
            "PURRDR_ADD_TO_PATH",
            "PURRDR_YES",
            "https://raychiutw.github.io/purrdr/host-service/#install-options",
            "https://raychiutw.github.io/purrdr/host-service/llms.txt",
        ] {
            assert!(stdout.contains(expected), "{flag} 缺少 {expected}:\n{stdout}");
        }
    }
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

/// 假的 `tailscale`(只放在 PATH 最前面):`version` 成功、`status --json` 回 `state`、`serve status --json` 回空。
/// 其他子命令一律失敗,所以測試若走到真的設定 serve 會馬上看到。
#[cfg(unix)]
fn fake_tailscale(directory: &TempDirectory, state: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = directory.0.join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("tailscale");
    let status = format!(
        r#"{{"BackendState":"{state}","Self":{{"DNSName":"studio.tail1234.ts.net."}},"CurrentTailnet":{{"MagicDNSEnabled":true}},"CertDomains":["studio.tail1234.ts.net"]}}"#
    );
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  'version ') echo 1.0;;\n  'status --json') echo '{status}';;\n  'serve status') echo '{{}}';;\n  *) exit 1;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// 在沒有終端機(stdin 是 /dev/null)的情況下跑安裝,PATH 最前面是假 tailscale。
#[cfg(unix)]
fn run_install_with_fake_tailscale(home: &TempDirectory, state: &str, arguments: &[&str]) -> Output {
    let holder = TempDirectory::new("binary-fake-tailscale-holder");
    let bin = fake_tailscale(&holder, state);
    // 假 tailscale 放在家目錄之外,家目錄才能驗證完全沒被動過。
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    Command::new(env!("CARGO_BIN_EXE_purrdr-host-service"))
        .args(arguments)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("PATH", path)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("CI")
        .env_remove("PURRDR_YES")
        .env_remove("PURRDR_HOST_SERVICE_INSTALL_COMMAND")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

#[cfg(unix)]
#[test]
fn install_without_a_terminal_and_without_yes_stops_unchanged_and_gives_the_rerun_with_yes() {
    let home = TempDirectory::new("binary-consent");

    let output = run_install_with_fake_tailscale(&home, "Running", &["install", "--port", "47999"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr.contains("studio.tail1234.ts.net"), "{stderr}");
    assert!(stderr.contains("install --port 47999 --yes"), "{stderr}");
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0, "家目錄不該被改動");
}

#[cfg(unix)]
#[test]
fn json_does_not_count_as_consent_and_prints_one_json_object_on_stdout() {
    let home = TempDirectory::new("binary-json-consent");

    let output = run_install_with_fake_tailscale(&home, "Running", &["install", "--json", "--port", "47999"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("stdout 不是單一 JSON:{error}\n{output:?}"));
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["kind"], "consent_required");
    let rerun = json["rerun_command"].as_str().unwrap();
    assert!(rerun.contains("--json") && rerun.contains("--yes") && rerun.contains("--port 47999"), "{rerun}");
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn json_failure_prints_the_error_and_the_rerun_command_as_json() {
    let home = TempDirectory::new("binary-json-failure");

    let output = run_install_with_fake_tailscale(&home, "NeedsLogin", &["install", "--json", "--yes"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("stdout 不是單一 JSON:{error}\n{output:?}"));
    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["kind"], "tailscale_not_running");
    assert!(json["rerun_command"].as_str().unwrap().contains("install --yes --json"), "{json}");
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);
}
