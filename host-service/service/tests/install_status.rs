//! 狀態檢查指令：回報主機服務、tailscale serve、herdr 的狀態(#62)。

mod install_support;
mod support;

use install_support::{FQDN, FakeSystem, TestHost, reserved_port};
use purrdr_host_service::herdr_socket::SocketEnvironment;
use purrdr_host_service::install::{HerdrStatus, InstallPlatform, Installer, ServeStatus};
use support::{FakeHerdr, recorded};

const PLATFORMS: [InstallPlatform; 3] = [InstallPlatform::MacOs, InstallPlatform::Linux, InstallPlatform::Windows];

fn socket_environment_at(path: &str) -> SocketEnvironment {
    SocketEnvironment {
        socket_path: Some(path.into()),
        ..SocketEnvironment::default()
    }
}

#[tokio::test]
async fn status_before_install_reports_nothing_installed() {
    let host = TestHost::new(InstallPlatform::current());
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));

    let status = installer.status(&socket_environment_at("purrdr-status-test-missing-socket")).await;

    assert!(!status.installed);
    assert!(!status.autostart_registered);
    assert!(!status.listening);
    assert_eq!(status.serve, ServeStatus::NotConfigured);
}

#[tokio::test]
async fn status_after_install_reports_autostart_and_serve_on_every_platform() {
    for platform in PLATFORMS {
        let host = TestHost::new(platform);
        let port = reserved_port();
        let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
        installer.install(&host.options(port), |_| true).unwrap();

        let status = installer.status(&socket_environment_at("purrdr-status-test-missing-socket")).await;

        assert!(status.installed, "{platform:?}");
        assert!(status.autostart_registered, "{platform:?}");
        assert_eq!(status.auto_update, Some(true), "{platform:?}");
        assert!(status.to_string().contains("  auto update: on\n"), "{status}");
        assert!(!status.listening, "{platform:?}");
        assert_eq!(status.https_address.as_deref(), Some(format!("https://{FQDN}").as_str()));
        assert_eq!(status.serve, ServeStatus::Configured(format!("http://127.0.0.1:{port}")), "{platform:?}");
        assert!(matches!(status.herdr, HerdrStatus::NotRunning(_)), "{platform:?}: {:?}", status.herdr);
    }
}

#[tokio::test]
async fn status_tells_where_the_logs_are_on_every_platform() {
    for platform in PLATFORMS {
        let host = TestHost::new(platform);
        let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
        installer.install(&host.options(reserved_port()), |_| true).unwrap();

        let status = installer.status(&socket_environment_at("purrdr-status-test-missing-socket")).await;

        let logs = host.service_directory().join("logs");
        assert!(status.to_string().contains(&format!("  logs: {}\n", logs.display())), "{platform:?}: {status}");
    }
}

#[tokio::test]
async fn status_reports_running_service_and_herdr() {
    let herdr = FakeHerdr::start(vec![("ping", recorded("ping.ndjson"))]).await;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let host = TestHost::new(InstallPlatform::current());
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(listener.local_addr().unwrap().port()), |_| true).unwrap();

    let status = installer.status(&socket_environment_at(herdr.socket.path())).await;

    assert!(status.listening);
    assert_eq!(status.herdr, HerdrStatus::Running { version: "0.9.5".into() });
    let report = status.to_string();
    assert!(report.contains("herdr: running (0.9.5)"), "{report}");
    assert!(report.contains(&format!("https://{FQDN}")), "{report}");
}

#[tokio::test]
async fn status_flags_serve_pointing_somewhere_else() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(reserved_port()), |_| true).unwrap();
    let mut system = FakeSystem::ready(&host.home.0);
    system
        .state
        .serve
        .insert(format!("{FQDN}:443"), [("/".to_owned(), "http://127.0.0.1:3000".to_owned())].into());
    let mut installer = Installer::new(host.environment(), system);

    let status = installer.status(&socket_environment_at("purrdr-status-test-missing-socket")).await;

    assert_eq!(status.serve, ServeStatus::Other("/ -> http://127.0.0.1:3000".into()));
}

#[tokio::test]
async fn status_reports_the_https_port_in_use() {
    let host = TestHost::new(InstallPlatform::Linux);
    let port = reserved_port();
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options_on_https_port(port, 8443), |_| true).unwrap();

    let status = installer.status(&socket_environment_at("purrdr-status-test-missing-socket")).await;

    assert_eq!(status.https_port, 8443);
    assert_eq!(status.serve, ServeStatus::Configured(format!("http://127.0.0.1:{port}")));
    let report = status.to_string();
    assert!(report.contains(&format!("HTTPS address: https://{FQDN}:8443")), "{report}");
    assert!(report.contains(&format!("tailscale serve: HTTPS 8443 -> http://127.0.0.1:{port}")), "{report}");
}

#[tokio::test]
async fn status_reports_the_windows_watchdog_only_where_it_exists() {
    for platform in PLATFORMS {
        let host = TestHost::new(platform);
        let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
        installer.install(&host.options(reserved_port()), |_| true).unwrap();

        let status = installer.status(&socket_environment_at("purrdr-status-test-missing-socket")).await;

        // 只登記了、看門狗沒有真的在跑(沒有 watchdog.json):不能說有看門狗在監看。
        let report = status.to_string();
        assert_eq!(
            report.contains("  auto restart: watchdog not running"),
            platform == InstallPlatform::Windows,
            "{platform:?}: {status}"
        );
        assert!(!report.contains("watchdog running"), "{platform:?}: {status}");
    }
}

#[tokio::test]
async fn status_follows_the_watchdog_state_file() {
    use purrdr_host_service::supervisor::{WatchdogRecord, WatchdogState, write_watchdog};
    let host = TestHost::new(InstallPlatform::Windows);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(reserved_port()), |_| true).unwrap();
    let now = purrdr_host_service::running_service::unix_millis(std::time::SystemTime::now());
    let socket = socket_environment_at("purrdr-status-test-missing-socket");
    let record = |state| WatchdogRecord {
        pid: std::process::id(),
        state,
        started_at_ms: now,
    };

    write_watchdog(&host.service_directory(), &record(WatchdogState::Running)).unwrap();
    let running = installer.status(&socket).await.to_string();
    write_watchdog(&host.service_directory(), &record(WatchdogState::GaveUp)).unwrap();
    let gave_up = installer.status(&socket).await.to_string();

    assert!(running.contains("  auto restart: watchdog running"), "{running}");
    assert!(gave_up.contains("  auto restart: watchdog gave up"), "{gave_up}");
}
