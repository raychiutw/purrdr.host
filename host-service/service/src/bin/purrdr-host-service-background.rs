//! Windows 登入時自動啟動的主機服務：與 `purrdr-host-service` 相同，但不開 console 視窗(#62)。

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // 與 `purrdr-host-service` 一樣:結束時不無限等 spawn_blocking 的工作。
    purrdr_host_service::shutdown::run_with_grace(purrdr_host_service::run_service(&arguments), std::time::Duration::from_secs(3))
}
