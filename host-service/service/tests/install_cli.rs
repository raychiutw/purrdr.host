//! 安裝相關子命令的參數、主機資訊與真正執行程式的系統介面(#62)。

use std::time::{Duration, Instant};

use purrdr_host_service::install::system::{HostSystem, ProcessHostSystem};
use purrdr_host_service::install::{HostEnvironment, InstallCommand, InstallError, InstallPlatform};

fn arguments(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn install_subcommands_are_parsed() {
    assert_eq!(
        InstallCommand::parse(&arguments("install")),
        Some(Ok(InstallCommand::Install {
            port: None,
            herdr_session: None,
            https_port: None,
            assume_yes: false,
            json: false,
            auto_update: None,
            add_to_path: None,
        }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("install --port 47999 --session work --yes")),
        Some(Ok(InstallCommand::Install {
            port: Some(47_999),
            herdr_session: Some("work".into()),
            https_port: None,
            assume_yes: true,
            json: false,
            auto_update: None,
            add_to_path: None,
        }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("install --https-port 8443")),
        Some(Ok(InstallCommand::Install {
            port: None,
            herdr_session: None,
            https_port: Some(8443),
            assume_yes: false,
            json: false,
            auto_update: None,
            add_to_path: None,
        }))
    );
    assert_eq!(InstallCommand::parse(&arguments("uninstall")), Some(Ok(InstallCommand::Uninstall)));
    assert_eq!(
        InstallCommand::parse(&arguments("update")),
        Some(Ok(InstallCommand::Update {
            check_only: false,
            assume_yes: false
        }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("update --check")),
        Some(Ok(InstallCommand::Update {
            check_only: true,
            assume_yes: false
        }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("update --yes")),
        Some(Ok(InstallCommand::Update {
            check_only: false,
            assume_yes: true
        }))
    );
    assert_eq!(InstallCommand::parse(&arguments("version")), Some(Ok(InstallCommand::Version)));
    assert_eq!(
        InstallCommand::parse(&arguments("install --no-auto-update")),
        Some(Ok(InstallCommand::Install {
            port: None,
            herdr_session: None,
            https_port: None,
            assume_yes: false,
            json: false,
            auto_update: Some(false),
            add_to_path: None,
        }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("install --no-path")),
        Some(Ok(InstallCommand::Install {
            port: None,
            herdr_session: None,
            https_port: None,
            assume_yes: false,
            json: false,
            auto_update: None,
            add_to_path: Some(false),
        }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("install --auto-update --path")),
        Some(Ok(InstallCommand::Install {
            port: None,
            herdr_session: None,
            https_port: None,
            assume_yes: false,
            json: false,
            auto_update: Some(true),
            add_to_path: Some(true),
        }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("auto-update on")),
        Some(Ok(InstallCommand::AutoUpdate { enabled: true }))
    );
    assert_eq!(
        InstallCommand::parse(&arguments("auto-update off")),
        Some(Ok(InstallCommand::AutoUpdate { enabled: false }))
    );
    assert_eq!(InstallCommand::parse(&arguments("status")), Some(Ok(InstallCommand::Status)));
}

#[test]
fn other_arguments_are_left_to_the_service_or_rejected() {
    assert_eq!(InstallCommand::parse(&[]), None);
    assert_eq!(InstallCommand::parse(&arguments("--port 47999")), None);
    for text in [
        "install --bind 0.0.0.0",
        "install --port",
        "install --port x",
        "install --https-port",
        "install --https-port 0",
        "install --https-port https",
        "install --auto-update --no-auto-update",
        "install --no-path --path",
        "uninstall --force",
        "status extra",
        "update --force",
        "update now",
        "version --check",
        "auto-update",
        "auto-update maybe",
        "auto-update on off",
    ] {
        assert!(matches!(InstallCommand::parse(&arguments(text)), Some(Err(_))), "{text}");
    }
}

#[test]
fn home_is_userprofile_on_windows_and_home_elsewhere() {
    let variables = |name: &str| match name {
        "HOME" => Some("/home/demo".to_owned()),
        "USERPROFILE" => Some(r"C:\Users\demo".to_owned()),
        "USERNAME" | "USER" => Some("demo".to_owned()),
        _ => None,
    };

    let windows = HostEnvironment::resolve(InstallPlatform::Windows, variables, "x.exe".into()).unwrap();
    let linux = HostEnvironment::resolve(InstallPlatform::Linux, variables, "x".into()).unwrap();
    let missing = HostEnvironment::resolve(InstallPlatform::MacOs, |_| None, "x".into());

    assert_eq!(windows.home, std::path::PathBuf::from(r"C:\Users\demo"));
    assert_eq!(linux.home, std::path::PathBuf::from("/home/demo"));
    assert_eq!(windows.user_name.as_deref(), Some("demo"));
    assert!(matches!(missing, Err(InstallError::HostEnvironment(_))));
}

#[test]
fn process_system_runs_programs_and_reports_missing_ones() {
    let mut system = ProcessHostSystem;
    let this_test_binary = std::env::current_exe().unwrap().display().to_string();

    let listed = system.run(&this_test_binary, &arguments("--list")).unwrap();
    let missing = system.run("purrdr-no-such-program", &[]);

    assert!(listed.success);
    assert!(
        listed.stdout.contains("process_system_runs_programs_and_reports_missing_ones"),
        "{}",
        listed.stdout
    );
    assert!(missing.is_err());
}

#[test]
fn process_system_removes_directory_once_this_process_lets_go() {
    let root = std::env::temp_dir().join(format!("purrdr-deferred-removal-{}", std::process::id()));
    let directory = root.join("host-service");
    std::fs::create_dir_all(directory.join("bin")).unwrap();
    std::fs::write(directory.join("bin/file"), "x").unwrap();

    ProcessHostSystem.remove_directory_after_exit(&directory, std::slice::from_ref(&root)).unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    while root.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(!root.exists(), "{} 還在", root.display());
}

#[test]
fn json_option_is_parsed_and_does_not_imply_yes() {
    let Some(Ok(InstallCommand::Install { json, assume_yes, .. })) = InstallCommand::parse(&arguments("install --json")) else {
        panic!("install --json 應該解析成功");
    };
    assert!(json);
    assert!(!assume_yes);
}
