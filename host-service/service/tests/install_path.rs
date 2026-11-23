//! 安裝時把主機服務加進使用者層級 PATH(raychiutw/purrdr.host#9):Windows 寫 `HKCU\Environment` 的 `Path`(假登錄),
//! macOS／Linux 在 shell 設定檔加標記區塊(暫存家目錄)。

mod install_support;

use install_support::{FakeSystem, TestHost, snapshot};

use purrdr_host_service::install::{InstallPlatform, Installer};

const PORT: u16 = 47_820;

#[test]
fn windows_install_appends_bin_to_user_path_once_and_uninstall_removes_it() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut system = FakeSystem::ready(&host.home.0);
    let before = r"%USERPROFILE%\.cargo\bin;C:\Tools";
    system.state.user_path = Some(before.to_owned());
    let mut installer = Installer::new(host.environment(), system);
    let bin = host.service_directory().join("bin").display().to_string();

    installer.install(&host.options(PORT), |_| true).unwrap();
    installer.install(&host.options(PORT), |_| true).unwrap();

    assert_eq!(installer.system().state.user_path, Some(format!("{before};{bin}")));
    installer.uninstall().unwrap();
    assert_eq!(installer.system().state.user_path.as_deref(), Some(before));
}

const BLOCK_START: &str = "# >>> purrdr-host-service >>>";

#[test]
fn zsh_profile_gets_one_marked_block_and_uninstall_restores_it() {
    let host = TestHost::new(InstallPlatform::MacOs);
    let mut environment = host.environment();
    environment.shell = Some("/bin/zsh".into());
    let profile = host.home.0.join(".zshrc");
    std::fs::write(&profile, "alias ll='ls -l'\n").unwrap();
    let mut installer = Installer::new(environment, FakeSystem::ready(&host.home.0));
    // 在 Windows CI 跑 macOS 流程時路徑有反斜線，雙引號內要跳脫。
    let bin = host.service_directory().join("bin").display().to_string().replace('\\', r"\\");

    installer.install(&host.options(PORT), |_| true).unwrap();
    installer.install(&host.options(PORT), |_| true).unwrap();

    let contents = std::fs::read_to_string(&profile).unwrap();
    assert_eq!(
        contents,
        format!("alias ll='ls -l'\n{BLOCK_START}\nexport PATH=\"{bin}:$PATH\"\n# <<< purrdr-host-service <<<\n")
    );
    assert_eq!(installer.system().state.user_path, None, "macOS 不碰 Windows 的使用者 PATH");
    installer.uninstall().unwrap();
    assert_eq!(std::fs::read_to_string(&profile).unwrap(), "alias ll='ls -l'\n");
}

#[test]
fn profile_follows_the_users_shell_and_new_profiles_are_removed_on_uninstall() {
    let cases = [
        (InstallPlatform::Linux, Some("/usr/bin/bash"), ".bashrc", "export PATH="),
        (InstallPlatform::MacOs, Some("/bin/bash"), ".bash_profile", "export PATH="),
        (InstallPlatform::Linux, Some("/usr/bin/fish"), ".config/fish/config.fish", "set -gx PATH "),
        (InstallPlatform::Linux, Some("/bin/dash"), ".profile", "export PATH="),
        (InstallPlatform::MacOs, None, ".profile", "export PATH="),
    ];
    for (platform, shell, profile, setting) in cases {
        let host = TestHost::new(platform);
        let home_before = snapshot(&host.home.0);
        let mut environment = host.environment();
        environment.shell = shell.map(str::to_owned);
        let mut installer = Installer::new(environment, FakeSystem::ready(&host.home.0));

        installer.install(&host.options(PORT), |_| true).unwrap();

        let contents = std::fs::read_to_string(host.home.0.join(profile)).unwrap_or_default();
        assert!(contents.contains(BLOCK_START) && contents.contains(setting), "{shell:?}: {contents}");
        installer.uninstall().unwrap();
        assert_eq!(snapshot(&host.home.0), home_before, "{shell:?}");
    }
}

#[test]
fn no_path_option_skips_path_and_is_kept_when_installing_again() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(PORT), |_| true).unwrap();
    let without_path = host.options_without_path(PORT);

    installer.install(&without_path, |_| true).unwrap();
    let after_option = installer.system().state.user_path.clone();
    installer.install(&host.options(PORT), |_| true).unwrap();

    assert_eq!(after_option, None, "--no-path 移除上次加的 PATH");
    assert_eq!(installer.system().state.user_path, None, "再次安裝沿用 --no-path");
}

#[test]
fn records_from_before_path_support_add_path_when_installing_again() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(PORT), |_| true).unwrap();
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    let record_file = host.service_directory().join("install.json");
    let mut record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&record_file).unwrap()).unwrap();
    for field in ["add_to_path", "path"] {
        record.as_object_mut().unwrap().remove(field);
    }
    std::fs::write(&record_file, record.to_string()).unwrap();

    installer.install(&host.options(PORT), |_| true).unwrap();

    let bin = host.service_directory().join("bin").display().to_string();
    assert_eq!(installer.system().state.user_path, Some(bin));
}

#[test]
fn installed_message_says_to_open_a_new_terminal_unless_no_path() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));

    let with_path = installer.install(&host.options(PORT), |_| true).unwrap().to_string();
    let without_path = host.options_without_path(PORT);
    let skipped = installer.install(&without_path, |_| true).unwrap().to_string();

    // 指令名稱(purrdr 或完整名稱)依別名有沒有建成，見 install_command_alias.rs。
    let hint = "Added to your PATH: open a new terminal to run";
    assert!(with_path.contains(hint), "{with_path}");
    assert!(!skipped.contains(hint), "{skipped}");
}

#[test]
fn block_start_without_its_end_keeps_the_rest_of_the_profile() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut environment = host.environment();
    environment.shell = Some("/bin/bash".into());
    let profile = host.home.0.join(".bashrc");
    let original = format!("alias ll='ls -l'\n{BLOCK_START}\nexport KEEP=1\n");
    std::fs::write(&profile, &original).unwrap();
    let mut installer = Installer::new(environment, FakeSystem::ready(&host.home.0));

    installer.install(&host.options(PORT), |_| true).unwrap();
    let installed = std::fs::read_to_string(&profile).unwrap();
    installer.uninstall().unwrap();

    assert!(installed.starts_with(&original), "{installed}");
    assert_eq!(std::fs::read_to_string(&profile).unwrap(), original);
}

#[test]
fn windows_path_entry_written_with_environment_variables_is_not_added_again() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut system = FakeSystem::ready(&host.home.0);
    let relative = host.service_directory().join("bin");
    let relative = relative.strip_prefix(&host.home.0).unwrap().display().to_string();
    let before = format!("%UserProfile%{}{relative};C:\\Tools", std::path::MAIN_SEPARATOR);
    system.state.user_path = Some(before.clone());
    let mut installer = Installer::new(host.environment(), system);

    installer.install(&host.options(PORT), |_| true).unwrap();
    let installed = installer.system().state.user_path.clone();
    installer.uninstall().unwrap();

    assert_eq!(installed.as_deref(), Some(before.as_str()));
    assert_eq!(installer.system().state.user_path.as_deref(), Some(before.as_str()));
}

#[test]
fn windows_user_path_longer_than_the_limit_is_not_written() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut system = FakeSystem::ready(&host.home.0);
    let before = format!("C:\\{}", "a".repeat(32_760));
    system.state.user_path = Some(before.clone());
    let mut installer = Installer::new(host.environment(), system);

    let failure = installer.install(&host.options(PORT), |_| true).unwrap_err().to_string();

    assert!(failure.contains("32767"), "{failure}");
    assert_eq!(installer.system().state.user_path.as_deref(), Some(before.as_str()));
}

/// 真的讀寫登錄，但用暫存機碼(不碰使用者的 PATH),結束時刪掉。
#[cfg(windows)]
#[test]
fn windows_registry_keeps_the_unexpanded_value_in_a_scratch_key() {
    use purrdr_host_service::install::system::windows_environment::{set_user_path, user_path};

    let parent = r"Software\PurrdrHostServiceTest";
    let subkey = format!(r"{parent}\{}", std::process::id());
    let value = r"%USERPROFILE%\.cargo\bin;C:\工具 資料夾\bin";

    let before = user_path(&subkey).unwrap();
    set_user_path(&subkey, Some(value)).unwrap();
    let written = user_path(&subkey).unwrap();
    let query = std::process::Command::new("reg")
        .args(["query", &format!(r"HKCU\{subkey}"), "/v", "Path"])
        .output()
        .unwrap();
    set_user_path(&subkey, None).unwrap();
    let removed = user_path(&subkey).unwrap();
    let _ = std::process::Command::new("reg").args(["delete", &format!(r"HKCU\{parent}"), "/f"]).output();

    assert_eq!(before, None);
    assert_eq!(written.as_deref(), Some(value));
    assert!(String::from_utf8_lossy(&query.stdout).contains("REG_EXPAND_SZ"));
    assert_eq!(removed, None);
}
