//! 主機服務的更新(raychiutw/purrdr.host#6、#7):假 Release 伺服器加暫存家目錄與假系統，三平台流程都在每個 CI 平台跑。

mod install_support;
mod support;

use std::time::Duration;

use install_support::release::{FakeReleaseServer, sha256_hex};
use install_support::{
    FakeSystem, LAUNCH_AGENT_LABEL, RUN_VALUE, SYSTEMD_UNIT, TEAM_ID, TestHost, executable_names, fake_host_service, reserved_port, snapshot,
};
use purrdr_host_service::agent_activity::HerdrAgentActivity;
use purrdr_host_service::install::{InstallPlatform, InstalledService, Installer};
use purrdr_host_service::update::{
    AgentActivity, AutomaticOutcome, HttpReleaseSource, InstallLocation, UpdateError, UpdateLauncher, UpdateOutcome, Updater, Version,
};
use support::{FakeHerdr, recorded, unique_socket};

const PLATFORMS: [InstallPlatform; 3] = [InstallPlatform::MacOs, InstallPlatform::Linux, InstallPlatform::Windows];
const INSTALLED: &str = "0.5.0";

fn version(text: &str) -> Version {
    text.parse().unwrap()
}

fn is_listening(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// 已安裝 0.5.0、背景服務執行中的測試主機。
struct InstalledHost {
    host: TestHost,
    port: u16,
    system: Option<FakeSystem>,
}

impl InstalledHost {
    fn new(platform: InstallPlatform) -> InstalledHost {
        let host = TestHost::new(platform);
        for name in executable_names(platform) {
            std::fs::write(host.downloads.0.join(name), fake_host_service(INSTALLED, name)).unwrap();
        }
        let port = reserved_port();
        let mut system = FakeSystem::ready(&host.home.0);
        system.service_port = Some(port);
        let mut installer = Installer::new(host.environment(), system);
        installer.install(&host.options(port), |_| true).unwrap();
        std::fs::write(host.service_directory().join("devices.json"), r#"{"devices":["paired"]}"#).unwrap();
        let mut system = installer.into_system();
        system.commands.clear();
        assert!(is_listening(port), "安裝後背景服務要在聽");
        InstalledHost {
            host,
            port,
            system: Some(system),
        }
    }

    /// 同一個家目錄的另一個安裝位置(例如另一個更新程序)。
    fn another_location(&self) -> InstalledService<FakeSystem> {
        InstalledService::new(self.host.environment(), FakeSystem::ready(&self.host.home.0))
    }

    fn registration(&self) -> Option<String> {
        let file = match self.host.platform {
            InstallPlatform::MacOs => self.host.home.0.join(format!("Library/LaunchAgents/{LAUNCH_AGENT_LABEL}.plist")),
            InstallPlatform::Linux => self.host.home.0.join(".config/systemd/user").join(SYSTEMD_UNIT),
            InstallPlatform::Windows => return None,
        };
        Some(std::fs::read_to_string(file).unwrap())
    }

    fn updater(&mut self, server: &FakeReleaseServer) -> Updater<HttpReleaseSource, InstalledService<FakeSystem>> {
        let location = InstalledService::new(self.host.environment(), self.system.take().unwrap()).with_start_timeout(Duration::from_secs(3));
        Updater::new(HttpReleaseSource::at(&server.url()), location)
    }

    fn installed_executable(&self, name: &str) -> String {
        std::fs::read_to_string(self.host.service_directory().join("bin").join(name)).unwrap()
    }

    /// 執行檔以外的設定與已配對裝置。
    fn settings(&self) -> Vec<String> {
        ["install.json", "config.json", "devices.json"]
            .iter()
            .map(|name| std::fs::read_to_string(self.host.service_directory().join(name)).unwrap())
            .collect()
    }
}

/// 家目錄快照，不含更新的紀錄與鎖(`update.json`、`update.lock`、`logs/update.log`)。
fn home_without_update_record(host: &TestHost) -> std::collections::BTreeMap<String, Option<String>> {
    let mut entries = snapshot(&host.home.0);
    entries.retain(|path, _| {
        !["update.json", "update.lock", "logs/update.log"]
            .iter()
            .any(|name| path.ends_with(&format!(".purrdr/host-service/{name}")))
    });
    entries
}

/// 記下被要求啟動更新程序幾次的假啟動器。
#[derive(Default)]
struct FakeLauncher {
    launches: usize,
    fails: bool,
}

impl UpdateLauncher for FakeLauncher {
    fn launch(&mut self) -> Result<(), String> {
        self.launches += 1;
        if self.fails { Err("spawn failed".into()) } else { Ok(()) }
    }
}

/// 固定回報有沒有 `working` Agent 的替身，記下被問幾次。
struct FakeAgents {
    working: bool,
    asked: usize,
}

impl FakeAgents {
    fn idle() -> FakeAgents {
        FakeAgents { working: false, asked: 0 }
    }

    fn working() -> FakeAgents {
        FakeAgents { working: true, asked: 0 }
    }
}

impl AgentActivity for FakeAgents {
    fn has_working_agent(&mut self) -> bool {
        self.asked += 1;
        self.working
    }
}

fn update_status(installed: &InstalledHost) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(installed.host.service_directory().join("update.json")).unwrap()).unwrap()
}

fn publish(server: &FakeReleaseServer, platform: InstallPlatform, version: &str, traits: &str) {
    let executables: Vec<(&str, String)> = executable_names(platform)
        .into_iter()
        .map(|name| (name, fake_host_service(version, &format!("{name} {traits}"))))
        .collect();
    server.publish(platform, version, &executables);
}

#[test]
fn newer_release_replaces_executables_restarts_service_and_keeps_settings_and_devices() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let settings_before = installed.settings();
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let mut updater = installed.updater(&server);

        let outcome = updater.apply().unwrap();

        assert_eq!(
            outcome,
            UpdateOutcome::Updated {
                from: version(INSTALLED),
                to: version("0.5.1")
            },
            "{platform:?}"
        );
        for name in executable_names(platform) {
            assert!(
                installed.installed_executable(name).starts_with("fake-host-service 0.5.1 "),
                "{platform:?} {name}"
            );
        }
        assert_eq!(installed.settings(), settings_before, "{platform:?}");
        assert!(is_listening(installed.port), "{platform:?}:更新後背景服務要重新啟動");
        let check = updater.check().unwrap();
        assert_eq!(
            (check.installed, check.latest.clone()),
            (Some(version("0.5.1")), Ok(version("0.5.1"))),
            "{platform:?}"
        );
        assert!(!check.update_available(), "{platform:?}");
    }
}

#[test]
fn same_or_older_release_changes_nothing() {
    for platform in PLATFORMS {
        for latest in [INSTALLED, "0.4.1"] {
            let mut installed = InstalledHost::new(platform);
            let server = FakeReleaseServer::start();
            publish(&server, platform, latest, "");
            let home_before = home_without_update_record(&installed.host);
            let mut updater = installed.updater(&server);

            let outcome = updater.apply().unwrap();

            assert_eq!(
                outcome,
                UpdateOutcome::UpToDate {
                    installed: version(INSTALLED),
                    latest: version(latest)
                },
                "{platform:?}"
            );
            assert_eq!(home_without_update_record(&installed.host), home_before, "{platform:?} {latest}");
            assert_eq!(server.requests(), ["/releases/latest"], "{platform:?} {latest}:不下載");
        }
    }
}

#[test]
fn checksum_mismatch_replaces_nothing() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let other_archive = format!("{}  {}\n", sha256_hex(b"something else"), install_support::release::asset_name(platform));
        server.put("releases/download/host-service-v0.5.1/SHA256SUMS", other_archive.into_bytes());
        let home_before = home_without_update_record(&installed.host);
        let mut updater = installed.updater(&server);

        let error = updater.apply().unwrap_err();

        assert!(matches!(error, UpdateError::ChecksumMismatch { .. }), "{platform:?}: {error:?}");
        assert!(error.explain("purrdr-host-service update").contains("Next:"), "{error}");
        assert_eq!(home_without_update_record(&installed.host), home_before, "{platform:?}");
        assert!(is_listening(installed.port), "{platform:?}");
    }
}

#[test]
fn new_version_that_does_not_start_is_rolled_back() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let settings_before = installed.settings();
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "broken");
        let mut updater = installed.updater(&server);

        let error = updater.apply().unwrap_err();

        assert!(
            matches!(&error, UpdateError::RolledBack { failed, restored, .. } if *failed == version("0.5.1") && *restored == version(INSTALLED)),
            "{platform:?}: {error:?}"
        );
        assert!(error.explain("purrdr-host-service update").contains("Next:"), "{error}");
        for name in executable_names(platform) {
            assert!(
                installed.installed_executable(name).starts_with("fake-host-service 0.5.0 "),
                "{platform:?} {name}"
            );
        }
        assert_eq!(installed.settings(), settings_before, "{platform:?}");
        assert!(is_listening(installed.port), "{platform:?}:退回後舊版要在跑");
        assert_eq!(updater.check().unwrap().installed, Some(version(INSTALLED)), "{platform:?}");
    }
}

#[test]
fn unreachable_release_server_is_a_network_error_and_changes_nothing() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let server = FakeReleaseServer::start();
        let home_before = home_without_update_record(&installed.host);
        let mut updater = installed.updater(&server);
        drop(server);

        let error = updater.apply().unwrap_err();

        assert!(matches!(error, UpdateError::Network(_)), "{platform:?}: {error:?}");
        assert!(error.explain("purrdr-host-service update").contains("Next:"), "{error}");
        assert_eq!(home_without_update_record(&installed.host), home_before, "{platform:?}");
    }
}

#[test]
fn check_keeps_installed_version_when_release_server_is_unreachable() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    let mut updater = installed.updater(&server);
    drop(server);

    let check = updater.check().unwrap();

    assert_eq!(check.installed, Some(version(INSTALLED)));
    assert!(matches!(check.latest, Err(UpdateError::Network(_))), "{check:?}");
    assert!(!check.update_available());
}

#[test]
fn missing_release_files_are_a_network_error() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    server.put("releases/latest", br#"{"tag_name":"host-service-v0.5.1"}"#.to_vec());
    let home_before = home_without_update_record(&installed.host);

    let error = installed.updater(&server).apply().unwrap_err();

    assert!(matches!(error, UpdateError::Network(_)), "{error:?}");
    assert_eq!(home_without_update_record(&installed.host), home_before);
}

#[test]
fn macos_release_without_matching_signature_is_not_installed() {
    let mut installed = InstalledHost::new(InstallPlatform::MacOs);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::MacOs, "0.5.1", "unsigned");
    let home_before = home_without_update_record(&installed.host);
    let mut updater = installed.updater(&server);

    let error = updater.apply().unwrap_err();

    assert!(matches!(error, UpdateError::Signature(_)), "{error:?}");
    assert!(error.explain("purrdr-host-service update").contains("Next:"), "{error}");
    assert_eq!(home_without_update_record(&installed.host), home_before);
    assert!(is_listening(installed.port));
}

#[test]
fn check_reports_installed_and_latest_versions_without_changes() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.10.0", "");
    let home_before = home_without_update_record(&installed.host);

    let check = installed.updater(&server).check().unwrap();

    assert_eq!((check.installed, check.latest.clone()), (Some(version(INSTALLED)), Ok(version("0.10.0"))));
    assert!(check.update_available());
    assert_eq!(home_without_update_record(&installed.host), home_before);
}

#[test]
fn update_without_install_says_how_to_install() {
    let host = TestHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let location = InstalledService::new(host.environment(), FakeSystem::ready(&host.home.0));
    let mut updater = Updater::new(HttpReleaseSource::at(&server.url()), location);

    let check = updater.check().unwrap();
    let error = updater.apply().unwrap_err();

    assert_eq!((check.installed, check.latest), (None, Ok(version("0.5.1"))));
    assert_eq!(error, UpdateError::NotInstalled);
    assert!(error.explain("purrdr-host-service update").contains("Next:"), "{error}");
    assert_eq!(std::fs::read_dir(&host.home.0).unwrap().count(), 0);
}

#[test]
fn versions_compare_numerically_and_parse_release_tags() {
    assert!(version("0.10.0") > version("0.9.9"));
    assert!(version("1.0.0") > version("0.99.99"));
    assert_eq!(version("0.5.0"), version("0.5.0"));
    assert_eq!(Version::from_tag("host-service-v0.5.1"), Some(version("0.5.1")));
    assert_eq!(Version::from_tag("v0.5.1"), None);
    for invalid in ["", "0.5", "0.5.x", "0.5.1-beta", "a.b.c"] {
        assert!(invalid.parse::<Version>().is_err(), "{invalid}");
    }
    assert_eq!(version("0.5.1").to_string(), "0.5.1");
}

#[test]
fn service_that_cannot_be_restarted_at_all_asks_to_reinstall_with_one_next_step() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        installed.system.as_mut().unwrap().autostart_fails = true;
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let mut updater = installed.updater(&server);

        let error = updater.apply().unwrap_err();

        assert!(matches!(error, UpdateError::RollbackFailed { .. }), "{platform:?}: {error:?}");
        let message = error.explain("purrdr-host-service update");
        assert_eq!(message.matches("Next:").count(), 1, "{message}");
        assert!(message.contains("install"), "{message}");
    }
}

#[test]
fn rollback_message_has_one_next_step() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "broken");
    let mut updater = installed.updater(&server);

    let message = updater.apply().unwrap_err().explain("purrdr-host-service update");

    assert_eq!(message.matches("Next:").count(), 1, "{message}");
}

#[test]
fn applying_records_the_result_and_the_check_reports_it() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut updater = installed.updater(&server);
    assert_eq!(updater.check().unwrap().last_result, None);

    updater.apply().unwrap();

    let check = updater.check().unwrap();
    assert_eq!(check.auto_update, Some(true));
    assert_eq!(check.last_result.unwrap().result, "updated 0.5.0 -> 0.5.1");
    let log = std::fs::read_to_string(installed.host.service_directory().join("logs/update.log")).unwrap();
    assert!(log.contains("updated 0.5.0 -> 0.5.1"), "{log}");
}

#[test]
fn automatic_check_launches_the_update_when_a_newer_release_exists() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let mut updater = installed.updater(&server);
        let mut launcher = FakeLauncher::default();

        let outcome = updater.automatic(&mut launcher, &mut FakeAgents::idle());

        assert_eq!(
            outcome,
            AutomaticOutcome::Started {
                from: version(INSTALLED),
                to: version("0.5.1")
            },
            "{platform:?}"
        );
        assert_eq!(launcher.launches, 1, "{platform:?}");
        assert!(
            installed
                .installed_executable(executable_names(platform)[0])
                .starts_with("fake-host-service 0.5.0 "),
            "由更新程序替換，不在服務裡換"
        );
        assert_eq!(updater.check().unwrap().last_result.unwrap().result, "update to 0.5.1 started");
    }
}

#[test]
fn automatic_check_does_nothing_when_auto_update_is_off_but_manual_update_still_works() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(installed.host.environment(), FakeSystem::ready(&installed.host.home.0));
    installer.set_auto_update(false).unwrap();
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher::default();

    let outcome = updater.automatic(&mut launcher, &mut FakeAgents::idle());

    assert_eq!(outcome, AutomaticOutcome::Disabled);
    assert_eq!(launcher.launches, 0);
    assert!(server.requests().is_empty(), "關閉時不連網路：{:?}", server.requests());
    assert_eq!(updater.check().unwrap().auto_update, Some(false));
    assert!(matches!(updater.apply(), Ok(UpdateOutcome::Updated { .. })));
}

#[test]
fn automatic_check_when_up_to_date_or_offline_launches_nothing_and_records_the_result() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, INSTALLED, "");
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher::default();

    let up_to_date = updater.automatic(&mut launcher, &mut FakeAgents::idle());
    assert_eq!(up_to_date, AutomaticOutcome::UpToDate(version(INSTALLED)));
    assert_eq!(updater.check().unwrap().last_result.unwrap().result, "up to date (0.5.0)");
    drop(server);
    let offline = updater.automatic(&mut launcher, &mut FakeAgents::idle());

    assert!(matches!(offline, AutomaticOutcome::Failed(UpdateError::Network(_))), "{offline:?}");
    assert_eq!(launcher.launches, 0);
    let last = updater.check().unwrap().last_result.unwrap().result;
    assert!(last.starts_with("check failed: "), "{last}");
}

#[test]
fn automatic_check_reports_a_launcher_that_cannot_start() {
    let mut installed = InstalledHost::new(InstallPlatform::Windows);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Windows, "0.5.1", "");
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher {
        fails: true,
        ..FakeLauncher::default()
    };

    let outcome = updater.automatic(&mut launcher, &mut FakeAgents::idle());

    assert!(matches!(outcome, AutomaticOutcome::Failed(UpdateError::Io(_))), "{outcome:?}");
}

#[test]
fn automatic_check_defers_the_update_while_an_agent_is_working() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher::default();
    let mut agents = FakeAgents::working();

    let outcome = updater.automatic(&mut launcher, &mut agents);

    assert_eq!(
        outcome,
        AutomaticOutcome::Deferred {
            to: version("0.5.1"),
            deferred: 1
        }
    );
    assert_eq!(launcher.launches, 0);
    let status = update_status(&installed);
    assert_eq!(status["deferred_count"], 1, "{status}");
    let result = status["result"].as_str().unwrap();
    assert!(result.contains("deferred") && result.contains("0.5.1"), "{result}");
}

#[test]
fn automatic_check_updates_anyway_after_three_deferrals_in_a_row_and_resets_the_count() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher::default();
    let mut agents = FakeAgents::working();

    let outcomes: Vec<AutomaticOutcome> = (0..4).map(|_| updater.automatic(&mut launcher, &mut agents)).collect();

    let deferrals: Vec<u32> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            AutomaticOutcome::Deferred { deferred, .. } => Some(*deferred),
            _ => None,
        })
        .collect();
    assert_eq!(deferrals, [1, 2, 3], "{outcomes:?}");
    assert!(matches!(outcomes[3], AutomaticOutcome::Started { .. }), "{outcomes:?}");
    assert_eq!(launcher.launches, 1);
    let status = update_status(&installed);
    assert!(status.get("deferred_count").is_none(), "照常更新後歸零：{status}");
}

#[test]
fn manual_update_does_not_wait_for_agents_and_the_next_check_ends_the_deferral_streak() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher::default();
    let mut agents = FakeAgents::working();
    assert!(matches!(updater.automatic(&mut launcher, &mut agents), AutomaticOutcome::Deferred { .. }));
    let asked_before_manual_update = agents.asked;

    let by_hand = updater.apply();
    let after_update = updater.automatic(&mut launcher, &mut agents);

    assert!(matches!(by_hand, Ok(UpdateOutcome::Updated { .. })), "{by_hand:?}");
    assert_eq!(after_update, AutomaticOutcome::UpToDate(version("0.5.1")));
    assert_eq!(agents.asked, asked_before_manual_update, "已是最新時不必問 Agent 狀態");
    let status = update_status(&installed);
    assert!(status.get("deferred_count").is_none(), "{status}");
}

#[test]
fn successful_manual_update_clears_the_deferral_count() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut updater = installed.updater(&server);
    assert!(matches!(
        updater.automatic(&mut FakeLauncher::default(), &mut FakeAgents::working()),
        AutomaticOutcome::Deferred { .. }
    ));

    let by_hand = updater.apply();

    assert!(matches!(by_hand, Ok(UpdateOutcome::Updated { .. })), "{by_hand:?}");
    let status = update_status(&installed);
    assert!(status.get("deferred_count").is_none(), "手動更新成功後歸零：{status}");
}

#[test]
fn automatic_update_skips_without_counting_a_deferral_while_a_manual_update_holds_the_reservation() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut manual_update = installed.another_location();
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher::default();
    assert!(matches!(
        updater.automatic(&mut launcher, &mut FakeAgents::working()),
        AutomaticOutcome::Deferred { deferred: 1, .. }
    ));
    manual_update.reserve_launch().unwrap();

    let while_reserved = updater.automatic(&mut launcher, &mut FakeAgents::idle());
    let status_while_reserved = update_status(&installed);
    manual_update.release_launch();
    let after_release = updater.automatic(&mut launcher, &mut FakeAgents::idle());

    assert_eq!(while_reserved, AutomaticOutcome::InProgress);
    assert_eq!(status_while_reserved["deferred_count"], 1, "略過不算延後：{status_while_reserved}");
    assert!(matches!(after_release, AutomaticOutcome::Started { .. }), "{after_release:?}");
    assert_eq!(launcher.launches, 1);
    assert_eq!(
        manual_update.reserve_launch(),
        Err(UpdateError::InProgress),
        "自動更新啟動的更新程序也占位，同時按立即更新會被擋"
    );
}

/// 以 herdr 的 `session.snapshot` 判斷 Agent 狀態，跑一次自動檢查(`snapshot` 為 None 時 herdr 沒在跑)。
fn automatic_check_with_herdr(snapshot: Option<serde_json::Value>) -> (AutomaticOutcome, usize) {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let (socket, _herdr) = match snapshot {
        Some(snapshot) => {
            let herdr = runtime.block_on(FakeHerdr::start(vec![("session.snapshot", snapshot)]));
            (herdr.socket.clone(), Some(herdr))
        }
        None => (unique_socket(), None),
    };
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let mut updater = installed.updater(&server);
    let mut launcher = FakeLauncher::default();
    let mut agents = HerdrAgentActivity::new(socket, runtime.handle().clone());

    let outcome = updater.automatic(&mut launcher, &mut agents);
    (outcome, launcher.launches)
}

/// 錄製的 snapshot 有 `working` 的 Agent;把它們都換成 idle。
fn snapshot_without_working_agents() -> serde_json::Value {
    let recorded = serde_json::to_string(&recorded("session.snapshot.ndjson")).unwrap();
    serde_json::from_str(&recorded.replace(r#""agent_status":"working""#, r#""agent_status":"idle""#)).unwrap()
}

#[test]
fn herdr_snapshot_with_a_working_agent_defers_the_automatic_update() {
    let (outcome, launches) = automatic_check_with_herdr(Some(recorded("session.snapshot.ndjson")));

    assert!(matches!(outcome, AutomaticOutcome::Deferred { deferred: 1, .. }), "{outcome:?}");
    assert_eq!(launches, 0);
}

#[test]
fn herdr_snapshot_without_working_agents_lets_the_automatic_update_start() {
    let (outcome, launches) = automatic_check_with_herdr(Some(snapshot_without_working_agents()));

    assert!(matches!(outcome, AutomaticOutcome::Started { .. }), "{outcome:?}");
    assert_eq!(launches, 1);
}

#[test]
fn unreachable_herdr_counts_as_idle_and_the_automatic_update_starts() {
    let (outcome, launches) = automatic_check_with_herdr(None);

    assert!(matches!(outcome, AutomaticOutcome::Started { .. }), "{outcome:?}");
    assert_eq!(launches, 1);
}

#[test]
fn only_the_installed_copy_runs_automatic_updates() {
    let installed = InstalledHost::new(InstallPlatform::Linux);
    let mut from_installed_copy = installed.host.environment();
    from_installed_copy.current_executable = installed.host.service_directory().join("bin").join("purrdr-host-service");

    let downloaded = InstalledService::new(installed.host.environment(), FakeSystem::ready(&installed.host.home.0));
    let in_place = InstalledService::new(from_installed_copy, FakeSystem::ready(&installed.host.home.0));
    let not_installed_host = TestHost::new(InstallPlatform::Linux);
    let not_installed = InstalledService::new(not_installed_host.environment(), FakeSystem::ready(&not_installed_host.home.0));

    assert!(!downloaded.runs_installed_copy());
    assert!(in_place.runs_installed_copy());
    assert!(!not_installed.runs_installed_copy());
}

/// 自動啟動的登記(檔案、launchd／systemd／Run 值)與服務都在。
fn assert_still_registered_and_running(
    updater: &Updater<HttpReleaseSource, InstalledService<FakeSystem>>,
    installed: &InstalledHost,
    registration: &Option<String>,
) {
    let platform = installed.host.platform;
    let system = updater.location().system();
    assert_eq!(&installed.registration(), registration, "{platform:?}:登記檔不變");
    match platform {
        InstallPlatform::MacOs => assert!(system.state.launchd_loaded.contains(LAUNCH_AGENT_LABEL)),
        InstallPlatform::Linux => assert!(system.state.systemd_enabled.contains(SYSTEMD_UNIT)),
        InstallPlatform::Windows => assert!(system.state.run_values.contains_key(RUN_VALUE)),
    }
    let removing = ["launchctl bootout", "systemctl --user disable", "reg delete", "reg add"];
    let commands = &system.commands;
    assert!(
        !commands.iter().any(|command| removing.iter().any(|prefix| command.starts_with(prefix))),
        "{platform:?}:重啟不取消登記：{commands:?}"
    );
    assert!(is_listening(installed.port), "{platform:?}:服務要在跑");
}

#[test]
fn update_restarts_in_place_without_unregistering_and_cleans_up_the_previous_executables() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let registration = installed.registration();
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let mut updater = installed.updater(&server);

        updater.apply().unwrap();

        assert_still_registered_and_running(&updater, &installed, &registration);
        let bin: Vec<String> = std::fs::read_dir(installed.host.service_directory().join("bin"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!bin.iter().any(|name| name.ends_with(".previous")), "{platform:?}:成功後清掉舊執行檔：{bin:?}");
        let leftovers: Vec<String> = std::fs::read_dir(installed.host.service_directory())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("update-"))
            .collect();
        assert!(leftovers.is_empty(), "{platform:?}:暫存資料夾沒清掉：{leftovers:?}");
    }
}

#[test]
fn rollback_restarts_in_place_and_keeps_the_service_registered() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let registration = installed.registration();
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "broken");
        let mut updater = installed.updater(&server);

        assert!(matches!(updater.apply(), Err(UpdateError::RolledBack { .. })), "{platform:?}");

        assert_still_registered_and_running(&updater, &installed, &registration);
    }
}

#[test]
fn failed_replacement_puts_the_old_executables_back_and_leaves_the_service_running() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let registration = installed.registration();
        let last = *executable_names(platform).last().unwrap();
        // 最後一個執行檔換不掉：它的 `.previous` 位置被一個資料夾占住。
        let obstacle = installed.host.service_directory().join("bin").join(format!("{last}.previous"));
        std::fs::create_dir_all(obstacle.join("in-the-way")).unwrap();
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let mut updater = installed.updater(&server);

        let error = updater.apply().unwrap_err();

        assert!(matches!(error, UpdateError::Io(_)), "{platform:?}: {error:?}");
        for name in executable_names(platform) {
            assert!(
                installed.installed_executable(name).starts_with("fake-host-service 0.5.0 "),
                "{platform:?} {name}"
            );
        }
        assert_still_registered_and_running(&updater, &installed, &registration);
    }
}

#[test]
fn restart_counts_only_when_the_running_service_reports_the_new_version() {
    for platform in PLATFORMS {
        let mut installed = InstalledHost::new(platform);
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "stale");
        let mut updater = installed.updater(&server);

        let error = updater.apply().unwrap_err();

        assert!(matches!(error, UpdateError::RolledBack { .. }), "{platform:?}: {error:?}");
        assert!(
            installed
                .installed_executable(executable_names(platform)[0])
                .starts_with("fake-host-service 0.5.0 ")
        );
    }
}

#[test]
fn an_intentional_restart_is_not_left_looking_like_a_crash() {
    // macOS 的 kickstart -k 是強制結束,服務記不到關閉紀錄,所以重啟的人要清掉 running.json;Linux 靠 SIGTERM 正常關閉。
    for platform in [InstallPlatform::MacOs, InstallPlatform::Linux] {
        let mut installed = InstalledHost::new(platform);
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let mut updater = installed.updater(&server);

        assert!(matches!(updater.apply(), Ok(UpdateOutcome::Updated { .. })), "{platform:?}");

        assert_eq!(
            updater.location().system().state.stale_marker_starts,
            0,
            "{platform:?}:重啟後新程序不該看到舊的 running.json"
        );
    }
}

#[test]
fn a_failed_macos_kickstart_leaves_running_json_so_a_later_crash_is_still_detected() {
    let mut installed = InstalledHost::new(InstallPlatform::MacOs);
    let marker = installed.host.service_directory().join("running.json");
    assert!(marker.is_file(), "安裝後服務要寫了 running.json");
    installed.system.as_mut().unwrap().autostart_fails = true;
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::MacOs, "0.5.1", "");
    let mut updater = installed.updater(&server);

    assert!(updater.apply().is_err());

    assert!(marker.is_file(), "kickstart 失敗、舊服務還在跑,running.json 不能被刪掉");
}

#[test]
fn only_one_update_runs_at_a_time() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let mut other_update = installed.another_location();
    let _held = other_update.lock().unwrap();
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");
    let home_before = home_without_update_record(&installed.host);
    let mut updater = installed.updater(&server);

    let error = updater.apply().unwrap_err();

    assert_eq!(error, UpdateError::InProgress);
    let message = error.explain("purrdr-host-service update");
    assert!(message.contains("already in progress") && message.contains("Next:"), "{message}");
    assert_eq!(home_without_update_record(&installed.host), home_before);
    drop(_held);
    assert!(matches!(updater.apply(), Ok(UpdateOutcome::Updated { .. })), "鎖放開後可以更新");
}

#[test]
fn every_update_unpacks_into_its_own_staging_folder() {
    let installed = InstalledHost::new(InstallPlatform::Linux);
    let location = installed.another_location();

    let (first, second) = (location.staging_directory(), location.staging_directory());

    assert_ne!(first, second);
    assert!(first.starts_with(installed.host.service_directory()), "{}", first.display());
}

#[test]
fn macos_requires_the_purrdr_team_even_when_the_installed_copy_is_unsigned() {
    let mut installed = InstalledHost::new(InstallPlatform::MacOs);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::MacOs, "0.5.1", "");
    let mut updater = installed.updater(&server);

    updater.apply().unwrap();

    let commands = &updater.location().system().commands;
    assert!(
        !commands.iter().any(|command| command.starts_with("codesign -dv")),
        "不從已安裝的執行檔取 Team ID:{commands:?}"
    );
    assert!(commands.iter().any(|command| command.contains(TEAM_ID)), "{commands:?}");
}

#[test]
fn a_rolled_back_version_is_not_retried_automatically_but_can_be_updated_by_hand() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "broken");
    let mut updater = installed.updater(&server);
    assert!(matches!(updater.apply(), Err(UpdateError::RolledBack { .. })));
    let mut launcher = FakeLauncher::default();

    let same_version = updater.automatic(&mut launcher, &mut FakeAgents::idle());
    let skipped_record = updater.check().unwrap().last_result.unwrap().result;
    let downloads_before = server.requests().len();
    let by_hand = updater.apply();
    let downloaded_again = server.requests().len() > downloads_before + 1;
    publish(&server, InstallPlatform::Linux, "0.5.2", "");
    let newer_version = updater.automatic(&mut launcher, &mut FakeAgents::idle());

    assert_eq!(same_version, AutomaticOutcome::Skipped(version("0.5.1")));
    assert!(skipped_record.contains("0.5.1") && skipped_record.contains("rolled back"), "{skipped_record}");
    assert!(matches!(by_hand, Err(UpdateError::RolledBack { .. })), "手動 update 仍會再試：{by_hand:?}");
    assert!(downloaded_again, "{:?}", server.requests());
    assert!(matches!(newer_version, AutomaticOutcome::Started { .. }), "{newer_version:?}");
    assert_eq!(launcher.launches, 1);
}

#[test]
fn downloads_do_not_ask_for_json() {
    let mut installed = InstalledHost::new(InstallPlatform::Linux);
    let server = FakeReleaseServer::start();
    publish(&server, InstallPlatform::Linux, "0.5.1", "");

    installed.updater(&server).apply().unwrap();

    for (path, headers) in server.request_headers() {
        let asks_for_json = headers.iter().any(|header| header.starts_with("accept:") && header.contains("json"));
        assert_eq!(asks_for_json, path == "/releases/latest", "{path}: {headers:?}");
    }
}

/// 壓縮檔裡多一個連結項目(指向壓縮檔裡的執行檔，解壓工具本身會照做)。
fn archive_with_link(platform: InstallPlatform, kind: tar::EntryType) -> Vec<u8> {
    use std::io::Write;
    let target = executable_names(platform)[0];
    if platform == InstallPlatform::Windows {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for name in executable_names(platform) {
            writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(fake_host_service("0.5.1", name).as_bytes()).unwrap();
        }
        writer.add_symlink("shortcut", target, zip::write::SimpleFileOptions::default()).unwrap();
        return writer.finish().unwrap().into_inner();
    }
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
    for name in executable_names(platform) {
        let contents = fake_host_service("0.5.1", name);
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, contents.as_bytes()).unwrap();
    }
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(0);
    builder.append_link(&mut header, "shortcut", target).unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn archives_with_links_are_refused() {
    let cases = [
        (InstallPlatform::MacOs, tar::EntryType::Symlink),
        (InstallPlatform::Linux, tar::EntryType::Symlink),
        (InstallPlatform::Linux, tar::EntryType::Link),
        (InstallPlatform::Windows, tar::EntryType::Symlink),
    ];
    for (platform, kind) in cases {
        let mut installed = InstalledHost::new(platform);
        let server = FakeReleaseServer::start();
        publish(&server, platform, "0.5.1", "");
        let archive = archive_with_link(platform, kind);
        let asset = install_support::release::asset_name(platform);
        server.put(&format!("releases/download/host-service-v0.5.1/{asset}"), archive.clone());
        server.put(
            "releases/download/host-service-v0.5.1/SHA256SUMS",
            format!("{}  {asset}\n", sha256_hex(&archive)).into_bytes(),
        );
        let home_before = home_without_update_record(&installed.host);
        let mut updater = installed.updater(&server);

        let error = updater.apply().unwrap_err();

        assert!(
            matches!(&error, UpdateError::Io(message) if message.contains("link")),
            "{platform:?}: {error:?}"
        );
        assert_eq!(home_without_update_record(&installed.host), home_before, "{platform:?}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_helper_needs_systemd_run_and_does_not_pass_the_release_source() {
    use purrdr_host_service::install::HelperLauncher;
    let scratch = install_support::TempDirectory::new("systemd-run");
    let record = scratch.0.join("arguments");
    let fake_systemd_run = scratch.0.join("systemd-run");
    std::fs::write(&fake_systemd_run, format!("#!/bin/sh\necho \"$@\" > {}\n", record.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake_systemd_run, std::fs::Permissions::from_mode(0o755)).unwrap();

    let missing = HelperLauncher::new("/bin/true".into(), scratch.0.join("no-systemd-run").display().to_string()).launch();
    let launched = HelperLauncher::new("/bin/true".into(), fake_systemd_run.display().to_string()).launch();

    assert!(missing.is_err(), "沒有 systemd-run 時不改用會被一起結束的方式");
    assert!(launched.is_ok(), "{launched:?}");
    let arguments = std::fs::read_to_string(record).unwrap();
    assert!(arguments.starts_with("--user --collect --quiet /bin/true update --yes"), "{arguments}");
    assert!(!arguments.contains("--setenv"), "{arguments}");
}
