//! `purrdr-host-service [--port <埠>] [--session <herdr session>] [--state-dir <資料夾>]`:執行主機服務。
//! `purrdr-host-service pair | devices | revoke …`:管理連線配對與裝置(見 `management`)。
//! `purrdr-host-service --version`:印出版本(一行安裝腳本與 app 的更新提示用，#198)。
//! `purrdr-host-service install [--port <埠>] [--session <herdr session>] [--yes]`、`uninstall`、`status`:安裝(見 `install`)。
//! `purrdr-host-service update [--check] [--yes]`、`version`:更新到最新 Release、顯示版本(見 `update`)。
//! `purrdr-host-service help`:印出可用指令。安裝時在 `bin/` 放的別名 `purrdr` 不帶參數時也印這份(#53)。

use std::process::ExitCode;
use std::time::SystemTime;

use purrdr_host_service::herdr_socket::{Platform, SocketEnvironment};
use purrdr_host_service::install::{self, InstallCommand};
use purrdr_host_service::management;

/// 可用指令(用別名 `purrdr` 的寫法)。
const HELP: &str = "\
usage: purrdr <command>  (same as purrdr-host-service <command>)

  purrdr pair [--read-only] [--address <https address>] [--wait]   show a pairing QR code
  purrdr devices                                                   list paired devices
  purrdr revoke <device id>                                        remove a paired device
  purrdr install [options]                                         install or reinstall (see Install options)
  purrdr status                                                    show the service status
  purrdr update [--check] [--yes]                                  update to the latest release
  purrdr auto-update on|off                                        turn automatic updates on or off
  purrdr version                                                   show the installed version
  purrdr uninstall                                                 remove the host service

pair, devices and revoke also take --state-dir <dir> (default ~/.purrdr/host-service).

Install options (each can also be set with an environment variable; an option wins over the variable):
  --port <port>             local port                    PURRDR_PORT
  --https-port <port>       HTTPS port (Tailscale Serve)  PURRDR_HTTPS_PORT
  --session <name>          herdr session                 PURRDR_SESSION
  --auto-update, --no-auto-update   automatic updates     PURRDR_AUTO_UPDATE=1|0
  --path, --no-path         add bin/ to your PATH         PURRDR_ADD_TO_PATH=1|0
  --yes                     do not ask for confirmation   PURRDR_YES=1
  --json                    print the result as JSON (implies --yes)
Order of precedence: option > environment variable > previous install > default.

All install options: https://raychiutw.github.io/purrdr/host-service/#install-options
For AI agents:       https://raychiutw.github.io/purrdr/host-service/llms.txt
";

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if matches!(arguments.as_slice(), [command] if command == "help" || command == "--help" || command == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    // 自動啟動以完整名稱不帶參數啟動服務;只打別名 `purrdr` 時是人在問怎麼用。
    if arguments.is_empty() && invoked_as_command_alias() {
        eprint!("{HELP}");
        return ExitCode::from(2);
    }
    if matches!(arguments.as_slice(), [flag] if flag == "--version" || flag == "-V") {
        println!("purrdr-host-service {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if let Some(command) = InstallCommand::parse(&arguments) {
        return match command {
            Ok(command) => install::run_command(command).await,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(2)
            }
        };
    }
    match arguments.first().map(String::as_str) {
        Some("pair" | "devices" | "revoke") => run_management(&arguments),
        _ => purrdr_host_service::run_service(&arguments).await,
    }
}

/// 以 `bin/` 的別名 `purrdr` 執行(symlink 保留 argv[0] 的名稱)。
fn invoked_as_command_alias() -> bool {
    std::env::args_os()
        .next()
        .and_then(|program| std::path::Path::new(&program).file_stem().map(|stem| stem == "purrdr"))
        .unwrap_or(false)
}

fn run_management(arguments: &[String]) -> ExitCode {
    match management::run(arguments, Platform::current(), &SocketEnvironment::from_process(), SystemTime::now()) {
        Ok(output) => {
            print!("{output}");
            wait_for_enter_if_asked(arguments);
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            wait_for_enter_if_asked(arguments);
            ExitCode::from(2)
        }
    }
}

/// `pair --wait`:herdr 的 popup 在指令結束時就關掉，成功或失敗都等使用者按 Enter,讓 QR code 或錯誤留在畫面上。
fn wait_for_enter_if_asked(arguments: &[String]) {
    use std::io::Write as _;
    if management::waits_for_enter(arguments) {
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().read_line(&mut String::new());
    }
}
