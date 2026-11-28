//! Windows 登入時自動啟動的主機服務：與 `purrdr-host-service` 相同，但不開 console 視窗(#62)。

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    // 沒有 `--supervised-child` 的是登入時啟動的那份:當看門狗,把真正的服務當子程序執行並在它消失後重啟(#112)。
    if arguments.first().map(String::as_str) == Some(purrdr_host_service::supervisor::CHILD_FLAG) {
        arguments.remove(0);
    } else if let Some(code) = purrdr_host_service::supervisor::run_watchdog(&arguments) {
        return code;
    }
    // 與 `purrdr-host-service` 一樣:結束時不無限等 spawn_blocking 的工作。
    purrdr_host_service::shutdown::run_with_grace(purrdr_host_service::run_service(&arguments), std::time::Duration::from_secs(3))
}
