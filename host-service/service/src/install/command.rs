//! `install`／`uninstall`／`status`／`update`／`version` 子命令：接上真正的系統與終端機。

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use super::interactive::{TerminalPrompter, install_interactively};
use super::location::InstalledService;
use super::settings::{RequestedSettings, assume_yes};
use super::system::ProcessHostSystem;
use super::{HostEnvironment, InstallCommand, InstallError, InstallOptions, InstallQuestion, Installer, rerun_install_command, shell_quoted};
use crate::herdr_socket::SocketEnvironment;
use crate::update::{HttpReleaseSource, UpdateError, UpdateOutcome, Updater};

/// 一行安裝腳本設定的環境變數：重跑安裝的指令(不含選項)。
pub const INSTALL_COMMAND_VARIABLE: &str = "PURRDR_HOST_SERVICE_INSTALL_COMMAND";

/// 執行子命令並回傳結束碼。
pub async fn run_command(command: InstallCommand) -> ExitCode {
    let environment = match HostEnvironment::from_process() {
        Ok(environment) => environment,
        Err(error) => return fail(&error),
    };
    if let InstallCommand::Update { check_only, assume_yes } = command {
        return run_update(environment, check_only, assume_yes);
    }
    if command == InstallCommand::Version {
        return run_version(environment);
    }
    if let InstallCommand::AutoUpdate { enabled } = command {
        return run_auto_update(environment, enabled);
    }
    let source_directory = environment.current_executable.parent().map(|parent| parent.to_path_buf()).unwrap_or_default();
    // 一行安裝腳本傳入重跑用的指令;直接執行時用這支執行檔。
    let rerun_prefix = std::env::var(INSTALL_COMMAND_VARIABLE)
        .ok()
        .filter(|prefix| !prefix.trim().is_empty())
        .unwrap_or_else(|| format!("{} install", shell_quoted(&environment.current_executable, environment.platform)));
    let rerun = rerun_install_command(&rerun_prefix, &command, environment.platform);
    let mut installer = Installer::new(environment, ProcessHostSystem);
    match command {
        InstallCommand::Install { .. } => {
            let environment = |name: &str| std::env::var(name).ok();
            let requested = match RequestedSettings::resolve(&command, environment) {
                Ok(requested) => requested,
                Err(error) => {
                    eprintln!("{}", error.explain(&rerun));
                    return ExitCode::FAILURE;
                }
            };
            let assume_yes = match assume_yes(&command, environment) {
                Ok(assume_yes) => assume_yes,
                Err(error) => {
                    eprintln!("{}", error.explain(&rerun));
                    return ExitCode::FAILURE;
                }
            };
            let options = InstallOptions { requested, source_directory };
            let confirm = |question: &InstallQuestion| {
                println!("{question}");
                let prompt = match question {
                    InstallQuestion::UseAnotherHttpsPort { alternative, .. } => format!("Use HTTPS {alternative}? [y/N] "),
                    InstallQuestion::CertificateTransparency { .. } => "Continue? [y/N] ".to_owned(),
                };
                if assume_yes {
                    println!("{prompt}yes (--yes)");
                    return true;
                }
                ask(&prompt)
            };
            // 有終端機且沒有 --yes:先摘要與選單;否則照設定直接安裝。
            let interactive = !assume_yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
            let result = if interactive {
                install_interactively(&mut installer, options, &mut TerminalPrompter)
            } else {
                installer.install(&options, confirm)
            };
            match result {
                Ok(installed) => {
                    for warning in &installed.warnings {
                        eprintln!("warning: {warning}");
                    }
                    println!("{installed}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{}", error.explain(&rerun));
                    ExitCode::FAILURE
                }
            }
        }
        InstallCommand::Uninstall => match installer.uninstall() {
            Ok(uninstalled) => {
                for warning in &uninstalled.warnings {
                    eprintln!("warning: {warning}");
                }
                println!("Uninstalled.");
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error),
        },
        InstallCommand::Update { .. } | InstallCommand::Version => unreachable!("前面已處理"),
        InstallCommand::AutoUpdate { .. } => unreachable!("前面已處理"),
        InstallCommand::Status => {
            println!("{}", installer.status(&SocketEnvironment::from_process()).await);
            ExitCode::SUCCESS
        }
    }
}

fn updater(environment: HostEnvironment) -> Updater<HttpReleaseSource, InstalledService<ProcessHostSystem>> {
    Updater::new(HttpReleaseSource::from_environment(), InstalledService::new(environment, ProcessHostSystem))
}

/// `version`:已安裝與最新的版本;取不到最新版也照樣成功。
fn run_version(environment: HostEnvironment) -> ExitCode {
    let update_command = format!("{} update", shell_quoted(&environment.current_executable, environment.platform));
    let check = match updater(environment).check() {
        Ok(check) => check,
        Err(error) => return fail_update(&error, &update_command),
    };
    match check.installed {
        Some(installed) => println!("installed: {installed}"),
        None => println!("installed: not installed"),
    }
    match &check.latest {
        Ok(latest) if check.update_available() => println!("latest: {latest} (update available, run: {update_command})"),
        Ok(latest) if check.installed.is_some() => println!("latest: {latest} (up to date)"),
        Ok(latest) => println!("latest: {latest}"),
        Err(UpdateError::Network(message)) => println!("latest: unknown ({message})"),
        Err(error) => println!("latest: unknown ({error})"),
    }
    if let Some(auto_update) = check.auto_update {
        println!("auto update: {}", if auto_update { "on" } else { "off" });
        match &check.last_result {
            Some(record) => println!("last check: {}, {}", humantime::format_rfc3339_seconds(record.checked_at), record.result),
            None => println!("last check: never"),
        }
    }
    ExitCode::SUCCESS
}

/// `auto-update on|off`;背景服務下一次檢查時就照新的設定。
fn run_auto_update(environment: HostEnvironment, enabled: bool) -> ExitCode {
    let update_command = format!("{} update", shell_quoted(&environment.current_executable, environment.platform));
    match Installer::new(environment, ProcessHostSystem).set_auto_update(enabled) {
        Ok(()) if enabled => {
            println!("Auto update is on. The host service checks for a new release every day and installs it.");
            ExitCode::SUCCESS
        }
        Ok(()) => {
            println!("Auto update is off. To update by hand, run: {update_command}");
            ExitCode::SUCCESS
        }
        Err(InstallError::NotInstalled) => fail_update(&UpdateError::NotInstalled, &update_command),
        Err(error) => fail(&error),
    }
}

/// `update [--check] [--yes]`。
fn run_update(environment: HostEnvironment, check_only: bool, assume_yes: bool) -> ExitCode {
    let update_command = format!("{} update", shell_quoted(&environment.current_executable, environment.platform));
    let mut rerun = update_command.clone();
    if check_only {
        rerun.push_str(" --check");
    }
    if assume_yes {
        rerun.push_str(" --yes");
    }
    let mut updater = updater(environment);
    let (installed, latest) = match updater.check().and_then(|check| check.versions()) {
        Ok(versions) => versions,
        Err(error) => return fail_update(&error, &rerun),
    };
    if latest <= installed {
        // `update` 也算一次檢查，經由套用記下結果;`--check` 不記。
        if !check_only && let Err(error) = updater.apply() {
            return fail_update(&error, &rerun);
        }
        println!("Up to date: {installed} (latest release {latest}).");
        return ExitCode::SUCCESS;
    }
    if check_only {
        println!("Update available: {installed} -> {latest}");
        println!("To update, run: {update_command}");
        return ExitCode::SUCCESS;
    }
    println!("Update the host service from {installed} to {latest}? It restarts, so connected devices reconnect after a moment.");
    let prompt = "Update? [y/N] ";
    let confirmed = if assume_yes {
        println!("{prompt}yes (--yes)");
        true
    } else {
        ask(prompt)
    };
    if !confirmed {
        println!();
        eprintln!("Update cancelled; nothing was changed.");
        return ExitCode::FAILURE;
    }
    match updater.apply() {
        Ok(UpdateOutcome::Updated { from, to }) => {
            println!("Updated {from} -> {to}. Settings and paired devices were kept.");
            ExitCode::SUCCESS
        }
        Ok(UpdateOutcome::UpToDate { installed, latest }) => {
            println!("Up to date: {installed} (latest release {latest}).");
            ExitCode::SUCCESS
        }
        Err(error) => fail_update(&error, &rerun),
    }
}

fn fail_update(error: &UpdateError, rerun: &str) -> ExitCode {
    eprintln!("{}", error.explain(rerun));
    ExitCode::FAILURE
}

fn fail(error: &InstallError) -> ExitCode {
    eprintln!("{error}");
    ExitCode::FAILURE
}

fn ask(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).is_ok() && matches!(answer.trim(), "y" | "Y" | "yes")
}
