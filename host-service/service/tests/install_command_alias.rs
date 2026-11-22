//! 安裝時在 `bin/` 放指令別名 `purrdr`,讓使用者打 `purrdr pair` 就好(raychiutw/purrdr.host#53)。

mod install_support;

use install_support::{FakeSystem, TestHost};

use purrdr_host_service::install::{CommandAlias, InstallPlatform, InstalledService, Installer};

const PORT: u16 = 47_820;

fn install(host: &TestHost) -> Installer<FakeSystem> {
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(PORT), |_| true).unwrap();
    installer
}

#[cfg(unix)]
#[test]
fn macos_and_linux_link_purrdr_to_the_installed_executable() {
    for platform in [InstallPlatform::MacOs, InstallPlatform::Linux] {
        let host = TestHost::new(platform);

        install(&host);

        let alias = host.service_directory().join("bin/purrdr");
        assert_eq!(std::fs::read_link(&alias).unwrap(), std::path::Path::new("purrdr-host-service"), "{platform:?}");
        assert_eq!(std::fs::read_to_string(&alias).unwrap(), "binary purrdr-host-service", "{platform:?}");
    }
}

#[test]
fn windows_adds_a_purrdr_cmd_that_forwards_arguments_and_shows_help_without_them() {
    let host = TestHost::new(InstallPlatform::Windows);

    install(&host);

    let script = std::fs::read_to_string(host.service_directory().join("bin").join("purrdr.cmd")).unwrap();
    assert_eq!(
        script,
        "@rem purrdr-host-service command alias\r\n\
         @if \"%~1\"==\"\" \"%~dp0purrdr-host-service.exe\" help & exit /b 2\r\n\
         @\"%~dp0purrdr-host-service.exe\" %*\r\n"
    );
    // 最後一行不能再接 `& exit /b`:不帶結束碼的 exit 會把錯誤碼蓋成 0,失敗的指令會被當成成功
    // (0.10.0 在 Windows PC 實測)。
}

#[test]
fn install_message_says_purrdr_pair_in_a_new_terminal() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));

    let message = installer.install(&host.options(PORT), |_| true).unwrap().to_string();

    assert!(message.contains("Open a new terminal and run: purrdr pair"), "{message}");
    assert!(message.contains("open a new terminal to run purrdr directly"), "{message}");
}

#[test]
fn without_path_the_install_message_keeps_the_full_command() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    let options = host.options_without_path(PORT);

    let message = installer.install(&options, |_| true).unwrap().to_string();

    assert!(!message.contains("run: purrdr pair"), "{message}");
    assert!(message.contains("purrdr-host-service.exe pair"), "{message}");
}

#[test]
fn a_purrdr_the_user_put_in_bin_is_left_alone_with_a_warning() {
    let host = TestHost::new(InstallPlatform::Windows);
    let bin = host.service_directory().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("purrdr.cmd"), "@echo mine\r\n").unwrap();
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));

    let installed = installer.install(&host.options(PORT), |_| true).unwrap();

    assert_eq!(std::fs::read_to_string(bin.join("purrdr.cmd")).unwrap(), "@echo mine\r\n");
    assert!(
        installed.warnings.iter().any(|warning| warning.contains("purrdr.cmd")),
        "{:?}",
        installed.warnings
    );
    assert!(!installed.to_string().contains("run: purrdr pair"), "{installed}");
}

#[test]
fn installing_again_keeps_one_alias_and_uninstall_removes_it() {
    let host = TestHost::new(InstallPlatform::Windows);
    let mut installer = install(&host);

    let again = installer.install(&host.options(PORT), |_| true).unwrap();
    assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    assert!(again.to_string().contains("run: purrdr pair"), "{again}");

    installer.uninstall().unwrap();
    assert!(!host.service_directory().join("bin").join("purrdr.cmd").exists());
}

/// 安裝好的那份執行檔啟動時的主機資訊(靠自動更新升級的主機不會重跑安裝)。
fn running_installed_copy(host: &TestHost) -> InstalledService<FakeSystem> {
    let mut environment = host.environment();
    environment.current_executable = host.service_directory().join("bin").join(install_support::executable_names(host.platform)[0]);
    InstalledService::new(environment, FakeSystem::ready(&host.home.0))
}

#[test]
fn installed_copy_adds_a_missing_alias_when_it_starts() {
    let host = TestHost::new(InstallPlatform::Windows);
    install(&host);
    let alias = host.service_directory().join("bin").join("purrdr.cmd");
    std::fs::remove_file(&alias).unwrap();

    let repaired = running_installed_copy(&host).repair_command_alias();

    assert_eq!(repaired, Some(Ok(CommandAlias::Created)));
    assert!(alias.exists());
    assert_eq!(running_installed_copy(&host).repair_command_alias(), Some(Ok(CommandAlias::Present)));
}

#[test]
fn a_copy_run_from_elsewhere_does_not_touch_the_installed_bin() {
    let host = TestHost::new(InstallPlatform::Windows);
    install(&host);
    let alias = host.service_directory().join("bin").join("purrdr.cmd");
    std::fs::remove_file(&alias).unwrap();

    let repaired = InstalledService::new(host.environment(), FakeSystem::ready(&host.home.0)).repair_command_alias();

    assert_eq!(repaired, None);
    assert!(!alias.exists());
}

#[cfg(unix)]
#[test]
fn installed_copy_leaves_a_link_to_another_purrdr_host_service_alone() {
    let host = TestHost::new(InstallPlatform::MacOs);
    install(&host);
    let alias = host.service_directory().join("bin").join("purrdr");
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink("/opt/homebrew/bin/purrdr-host-service", &alias).unwrap();

    let repaired = running_installed_copy(&host).repair_command_alias();

    assert_eq!(repaired, Some(Ok(CommandAlias::Taken(alias.clone()))));
    assert_eq!(
        std::fs::read_link(&alias).unwrap(),
        std::path::Path::new("/opt/homebrew/bin/purrdr-host-service")
    );
}

#[test]
fn installed_copy_rewrites_an_older_purrdr_cmd_of_its_own() {
    let host = TestHost::new(InstallPlatform::Windows);
    install(&host);
    let alias = host.service_directory().join("bin").join("purrdr.cmd");
    let current = std::fs::read_to_string(&alias).unwrap();
    std::fs::write(&alias, "@rem purrdr-host-service command alias\r\n@\"%~dp0old.exe\" %*\r\n").unwrap();

    let repaired = running_installed_copy(&host).repair_command_alias();

    assert_eq!(repaired, Some(Ok(CommandAlias::Created)));
    assert_eq!(std::fs::read_to_string(&alias).unwrap(), current);
}
