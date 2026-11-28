//! 以命令列參數執行主機服務(`purrdr-host-service` 與 Windows 的 `purrdr-host-service-background` 共用)。

use std::io::IsTerminal;
use std::process::ExitCode;

use crate::herdr_socket::{Platform, SocketEnvironment};
use crate::{HostServiceConfig, WEBSOCKET_PATH};

/// `[--port <埠>] [--session <herdr session>] [--state-dir <資料夾>]`:啟動並一直執行。
pub async fn run_service(arguments: &[String]) -> ExitCode {
    let config = match HostServiceConfig::from_arguments(arguments, Platform::current(), &SocketEnvironment::from_process()) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let herdr_socket = config.herdr_socket.clone();
    let state_directory = config.state_directory.clone();
    let log = crate::service_log::ServiceLog::under(&state_directory);
    crate::shutdown::install_panic_hook(log.clone());
    crate::shutdown::report_unclean_previous_run(&state_directory, &log, std::process::id(), &crate::shutdown::SystemProcesses);
    let service = match crate::start(config).await {
        Ok(service) => service,
        Err(error) => {
            log.record(&format!("cannot listen: {error}"));
            return ExitCode::FAILURE;
        }
    };
    // 給人看的啟動訊息只在終端機輸出;log 檔裡已經有 start() 記的那一行。
    if std::io::stdout().is_terminal() {
        println!(
            "purrdr-host-service {} listening on http://{} (WebSocket {WEBSOCKET_PATH}, herdr socket {})",
            env!("CARGO_PKG_VERSION"),
            service.local_addr(),
            herdr_socket.native_name()
        );
    }
    if let Err(error) = crate::running_service::write(&state_directory, service.local_addr().port()) {
        log.record(&format!("cannot write running.json: {error}"));
    }
    repair_command_alias(&log);
    // 安裝好的那份才會自動更新(開發中直接執行的不會)。
    let _auto_update = crate::auto_update::spawn_for_installed_service(herdr_socket, service.plugin_updater());
    let reason = tokio::select! {
        reason = crate::shutdown::wait_for_termination() => reason,
        () = service.wait() => "the listener loop ended",
    };
    crate::shutdown::record_shutdown(&state_directory, &log, reason);
    ExitCode::SUCCESS
}

/// 安裝好的那份啟動時補上指令別名 `purrdr`;沒補成只記 log,不影響服務。
fn repair_command_alias(log: &crate::service_log::ServiceLog) {
    use crate::install::{CommandAlias, HostEnvironment, InstalledService, system::ProcessHostSystem};
    let environment = match HostEnvironment::from_process() {
        Ok(environment) => environment,
        Err(error) => return log.record(&format!("the purrdr command was not checked: {error}")),
    };
    match InstalledService::new(environment, ProcessHostSystem).repair_command_alias() {
        Some(Ok(CommandAlias::Taken(file))) => log.record(&format!("the purrdr command was not added: {} already exists", file.display())),
        Some(Err(error)) => log.record(&format!("the purrdr command was not added: {error}")),
        _ => {}
    }
}
