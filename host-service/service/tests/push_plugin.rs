//! 推播外掛代裝(raychiutw/purrdr.host#10、raychiutw/purrdr.ios#261):`push.status`／`push.install`／`push.remove`;
//! 測試通知 `push.test`(raychiutw/purrdr.host#60)對假後台(`install_support::push_backend`)送。
//! 以真正的 WebSocket client 從協定這一端驗證，外部行為只看回應、家目錄的檔案與假 herdr CLI 收到的指令。
//!
//! 假的 herdr CLI 就是這支測試執行檔本身(`fake-herdr <資料夾>` 參數):`installed` 存在代表已安裝 purrdr.push,
//! `install-fails`／`list-fails` 存在時對應指令失敗，每次呼叫的參數記在 `calls`。所以不用 libtest(`harness = false`)。

mod install_support;
mod support;

use std::collections::BTreeMap;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use install_support::push_backend::FakePushBackend;
use purrdr_host_service::herdr_cli::HerdrCli;
use purrdr_host_service::{PluginUpdate, PluginUpdater};
use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, TestHostService, recorded, start_service_with_herdr_cli_and_home};
use tempfile::TempDir;

const FAKE_HERDR: &str = "fake-herdr";
const PLUGIN_SOURCE: &str = "raychiutw/purrdr.host/host-plugin";
const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const ENDPOINT: &str = "https://push.example.com/v1/push";

// MARK: - 假的 herdr CLI

/// `fake-herdr <資料夾> [--session <名稱>] plugin list|install|uninstall …`(輸出形狀同 herdr 0.9.5)。
fn fake_herdr(arguments: &[String]) -> i32 {
    let directory = PathBuf::from(&arguments[0]);
    let herdr_arguments = &arguments[1..];
    let mut calls = std::fs::OpenOptions::new().create(true).append(true).open(directory.join("calls")).unwrap();
    writeln!(calls, "{}", herdr_arguments.join(" ")).unwrap();
    let installed = directory.join("installed");
    let plugin = herdr_arguments.iter().position(|argument| argument == "plugin").unwrap();
    match herdr_arguments[plugin..].join(" ").as_str() {
        "plugin list --plugin purrdr.push --json" => {
            if directory.join("list-fails").exists() {
                eprintln!("herdr server is not running");
                return 1;
            }
            let plugins = if installed.exists() {
                json!([{"plugin_id": "purrdr.push", "name": "Purrdr Push", "version": "0.1.0", "enabled": true}])
            } else {
                json!([])
            };
            println!("{}", json!({"id": "cli:plugin", "result": {"plugins": plugins, "type": "plugin_list"}}));
            0
        }
        "plugin install raychiutw/purrdr.host/host-plugin --yes" => {
            if directory.join("install-fails").exists() {
                eprintln!("git clone failed");
                return 1;
            }
            std::fs::write(installed, b"").unwrap();
            0
        }
        "plugin update purrdr.push --yes" => {
            if directory.join("update-fails").exists() {
                eprintln!("git pull failed");
                return 1;
            }
            0
        }
        "plugin uninstall purrdr.push" => {
            let _ = std::fs::remove_file(installed);
            0
        }
        other => {
            eprintln!("unexpected: {other}");
            2
        }
    }
}

// MARK: - 測試環境

struct Harness {
    _herdr: FakeHerdr,
    fake: TempDir,
    home: TempDir,
    service: TestHostService,
}

impl Harness {
    async fn start() -> Harness {
        Harness::start_with_session(None).await
    }

    async fn start_with_session(session: Option<&str>) -> Harness {
        let herdr = FakeHerdr::start(vec![("ping", recorded("ping.ndjson"))]).await;
        let fake = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        // 實際的主機上 `~/.purrdr/` 已有主機服務自己的資料夾。
        std::fs::create_dir_all(home.path().join(".purrdr/host-service")).unwrap();
        let mut cli = HerdrCli::at_path(
            std::env::current_exe().unwrap(),
            vec![FAKE_HERDR.to_owned(), fake.path().to_string_lossy().into_owned()],
        );
        if let Some(session) = session {
            cli = cli.with_session(session);
        }
        let service = start_service_with_herdr_cli_and_home(herdr.socket.clone(), cli, home.path().to_owned()).await;
        Harness {
            _herdr: herdr,
            fake,
            home,
            service,
        }
    }

    async fn client(&self) -> TestClient {
        let mut client = TestClient::connect(&self.service).await;
        client.request(hello()).await;
        client
    }

    async fn read_only_client(&self) -> TestClient {
        let device = self.service.pair_device("借出的 iPad", true);
        let mut client = TestClient::connect_with_token(&self.service, Some(&device.device_token)).await;
        client.request(hello()).await;
        client
    }

    /// 假 herdr 收到的指令(不含 `fake-herdr <資料夾>`)。
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.fake.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn plugin_installed(&self) -> bool {
        self.fake.path().join("installed").exists()
    }

    fn mark(&self, name: &str) {
        std::fs::write(self.fake.path().join(name), b"").unwrap();
    }

    /// 直接寫一支裝置的配對檔(endpoint 指向假後台，是 http,所以不經 push.install)。
    fn write_pairing(&self, device_id: &str, endpoint: &str, key: &str) {
        let directory = self.home.path().join(format!(".purrdr/push-{device_id}"));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("pairing"), format!("purrdr-push 1\nendpoint={endpoint}\nkey={key}\n")).unwrap();
    }

    fn home_file(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.home.path().join(relative)).ok()
    }

    /// 家目錄的樣子(相對路徑 → 內容;資料夾是 `None`)。
    fn home_snapshot(&self) -> BTreeMap<String, Option<String>> {
        let mut snapshot = BTreeMap::new();
        collect(self.home.path(), self.home.path(), &mut snapshot);
        snapshot
    }
}

fn collect(root: &Path, directory: &Path, snapshot: &mut BTreeMap<String, Option<String>>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            snapshot.insert(relative, None);
            collect(root, &path, snapshot);
        } else {
            snapshot.insert(relative, Some(std::fs::read_to_string(&path).unwrap()));
        }
    }
}

fn hello() -> Value {
    json!({"type": "hello", "request_id": "h1", "protocol_version": 1})
}

fn status(device_id: &str) -> Value {
    json!({"type": "push.status", "request_id": "s1", "device_id": device_id})
}

fn install(device_id: &str) -> Value {
    json!({
        "type": "push.install", "request_id": "i1", "device_id": device_id,
        "pairing": {"endpoint": ENDPOINT, "key": KEY, "muted": ["w1-session"]},
    })
}

fn remove(device_id: &str) -> Value {
    json!({"type": "push.remove", "request_id": "r1", "device_id": device_id})
}

/// 沒有 Agent 觀察者的主機服務(這支測試的預設):不會直接送，`direct_push` 是 false。
/// 靜音更新不問 herdr,回覆不帶 `direct_push`。
fn state(kind: &str, request_id: &str, plugin_installed: bool, paired: bool, other_devices: u64) -> Value {
    let mut reply = json!({
        "type": kind, "request_id": request_id, "plugin_installed": plugin_installed,
        "paired": paired, "other_devices": other_devices,
    });
    if kind != "push.update_muted.result" {
        reply["direct_push"] = json!(false);
    }
    reply
}

fn list_call(session: &str) -> String {
    format!("{session}plugin list --plugin purrdr.push --json")
}

// MARK: - 測試

async fn status_of_a_fresh_host_reports_nothing_installed() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;

    let reply = client.request(status("phone-a")).await;

    assert_eq!(reply, state("push.status.result", "s1", false, false, 0));
}

async fn install_writes_the_pairing_and_installs_the_plugin_without_session() {
    let harness = Harness::start_with_session(Some("work")).await;
    let mut client = harness.client().await;

    let reply = client.request(install("phone-a")).await;

    assert_eq!(reply, state("push.install.result", "i1", true, true, 0));
    assert_eq!(
        harness.home_file(".purrdr/push-phone-a/pairing").as_deref(),
        Some(format!("purrdr-push 1\nendpoint={ENDPOINT}\nkey={KEY}\nmuted=w1-session\n").as_str())
    );
    // SSH 路徑以安裝紀錄判斷已安裝(ReversibleHostWrite),兩條路徑要能互相移除。
    assert_eq!(
        harness.home_file(".purrdr/push-phone-a/journal.json").as_deref(),
        Some(r#"{"entries":[],"version":1}"#)
    );
    assert_eq!(
        harness.home_file(".purrdr/push-plugin/journal.json").as_deref(),
        Some(r#"{"entries":[],"version":1}"#)
    );
    assert_eq!(
        harness.home_file(".purrdr/push-plugin/source").as_deref(),
        Some(format!("{PLUGIN_SOURCE}\n").as_str())
    );
    assert!(harness.plugin_installed());
    assert_eq!(
        harness.calls(),
        [
            list_call("--session work "),
            format!("plugin install {PLUGIN_SOURCE} --yes"),
            list_call("--session work ")
        ]
    );
}

async fn second_device_only_adds_its_pairing() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    let installs_before = harness.calls().iter().filter(|call| call.contains("install")).count();

    let reply = client.request(install("ipad-b")).await;
    let status_of_first = client.request(status("phone-a")).await;

    assert_eq!(reply, state("push.install.result", "i1", true, true, 1));
    assert_eq!(status_of_first, state("push.status.result", "s1", true, true, 1));
    assert!(harness.home_file(".purrdr/push-ipad-b/pairing").is_some());
    assert_eq!(harness.calls().iter().filter(|call| call.contains("install")).count(), installs_before);
}

async fn failed_plugin_install_leaves_the_home_as_before() {
    let harness = Harness::start().await;
    harness.mark("install-fails");
    let before = harness.home_snapshot();
    let mut client = harness.client().await;

    let reply = client.request(install("phone-a")).await;

    assert_eq!(reply["type"], "error");
    assert_eq!(reply["request_id"], "i1");
    assert_eq!(reply["code"], "push_plugin_failed");
    assert_eq!(reply["message"], "git clone failed");
    assert_eq!(harness.home_snapshot(), before);
}

async fn herdr_failing_to_list_plugins_is_reported() {
    let harness = Harness::start().await;
    harness.mark("list-fails");
    let mut client = harness.client().await;

    let reply = client.request(status("phone-a")).await;

    assert_eq!(reply["code"], "push_plugin_failed");
    assert_eq!(reply["message"], "herdr server is not running");
}

async fn removing_one_device_keeps_the_plugin_for_the_others() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    client.request(install("ipad-b")).await;

    let reply = client.request(remove("phone-a")).await;

    assert_eq!(reply, state("push.remove.result", "r1", true, false, 1));
    assert_eq!(harness.home_file(".purrdr/push-phone-a/pairing"), None);
    assert!(!harness.home.path().join(".purrdr/push-phone-a").exists());
    assert!(harness.home_file(".purrdr/push-ipad-b/pairing").is_some());
    assert!(harness.plugin_installed());
    assert!(!harness.calls().iter().any(|call| call.contains("uninstall")));
}

async fn removing_the_last_device_uninstalls_the_plugin_and_restores_the_home() {
    let harness = Harness::start().await;
    let before = harness.home_snapshot();
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    client.request(install("ipad-b")).await;
    client.request(remove("ipad-b")).await;

    let reply = client.request(remove("phone-a")).await;

    assert_eq!(reply, state("push.remove.result", "r1", false, false, 0));
    assert!(!harness.plugin_installed());
    assert_eq!(harness.home_snapshot(), before);
}

async fn plugin_the_user_installed_is_never_uninstalled() {
    let harness = Harness::start().await;
    harness.mark("installed");
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;

    let reply = client.request(remove("phone-a")).await;

    assert_eq!(reply, state("push.remove.result", "r1", true, false, 0));
    assert!(harness.plugin_installed());
    assert!(!harness.calls().iter().any(|call| call.contains("install ") || call.contains("uninstall")));
}

async fn read_only_device_can_query_but_not_install_or_remove() {
    let harness = Harness::start().await;
    let before = harness.home_snapshot();
    let mut client = harness.read_only_client().await;

    let installed = client.request(install("phone-a")).await;
    let removed = client.request(remove("phone-a")).await;
    let calls_before_status = harness.calls();
    let queried = client.request(status("phone-a")).await;

    assert_eq!(installed["code"], "read_only_device");
    assert_eq!(removed["code"], "read_only_device");
    assert!(calls_before_status.is_empty(), "{calls_before_status:?}");
    assert_eq!(queried, state("push.status.result", "s1", false, false, 0));
    assert_eq!(harness.home_snapshot(), before);
}

async fn unsafe_device_ids_and_pairings_are_rejected_without_touching_the_host() {
    let harness = Harness::start().await;
    let before = harness.home_snapshot();
    let mut client = harness.client().await;
    let mut bad_key = install("phone-a");
    bad_key["pairing"]["key"] = json!("not-hex");
    let mut plain_http = install("phone-a");
    plain_http["pairing"]["endpoint"] = json!("http://push.example.com/v1/push");
    let mut multiline_mute = install("phone-a");
    multiline_mute["pairing"]["muted"] = json!(["a\nkey=evil"]);

    let replies = [
        client.request(install("../escape")).await,
        client.request(install("plugin")).await,
        client.request(remove("Phone")).await,
        client.request(status("")).await,
        client.request(bad_key).await,
        client.request(plain_http).await,
        client.request(multiline_mute).await,
    ];

    for reply in &replies {
        assert_eq!(reply["code"], "invalid_message", "{reply}");
    }
    assert!(harness.calls().is_empty(), "{:?}", harness.calls());
    assert_eq!(harness.home_snapshot(), before);
}

async fn install_and_remove_are_audited_without_the_key() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    client.request(remove("phone-a")).await;

    let audit = harness.service.audit_log();

    assert!(audit.contains("push.install"), "{audit}");
    assert!(audit.contains("push.remove"), "{audit}");
    assert!(!audit.contains(KEY), "{audit}");
}

async fn failed_reinstall_restores_the_previous_pairing() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    let paired = harness.home_snapshot();
    std::fs::remove_file(harness.fake.path().join("installed")).unwrap();
    harness.mark("install-fails");
    let mut renewed = install("phone-a");
    renewed["pairing"]["key"] = json!("ff".repeat(32));

    let reply = client.request(renewed).await;

    assert_eq!(reply["code"], "push_plugin_failed", "{reply}");
    assert_eq!(harness.home_snapshot(), paired);
}

async fn reinstall_overwrites_the_key_without_reinstalling_the_plugin() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    let mut renewed = install("phone-a");
    renewed["pairing"]["key"] = json!("ff".repeat(32));

    let reply = client.request(renewed).await;

    assert_eq!(reply, state("push.install.result", "i1", true, true, 0));
    let pairing = harness.home_file(".purrdr/push-phone-a/pairing").unwrap();
    assert!(pairing.contains(&format!("key={}\n", "ff".repeat(32))), "{pairing}");
    assert_eq!(harness.calls().iter().filter(|call| call.contains("install ")).count(), 1);
}

fn update_muted(device_id: &str, muted: Value) -> Value {
    json!({"type": "push.update_muted", "request_id": "m1", "device_id": device_id, "muted": muted})
}

async fn update_muted_rewrites_only_muted_without_herdr() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    let calls_before = harness.calls();
    harness.mark("list-fails");

    let reply = client.request(update_muted("phone-a", json!(["w2-session", "w1-session", "w2-session"]))).await;
    let cleared = client.request(update_muted("phone-a", json!([]))).await;

    assert_eq!(reply, state("push.update_muted.result", "m1", true, true, 0));
    assert_eq!(cleared, state("push.update_muted.result", "m1", true, true, 0));
    assert_eq!(
        harness.home_file(".purrdr/push-phone-a/pairing").as_deref(),
        Some(format!("purrdr-push 1\nendpoint={ENDPOINT}\nkey={KEY}\n").as_str())
    );
    assert_eq!(harness.calls(), calls_before, "不呼叫 herdr");
}

async fn update_muted_keeps_key_and_endpoint_and_sorts_sessions() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;

    client.request(update_muted("phone-a", json!(["w2-session", "w1-session"]))).await;

    assert_eq!(
        harness.home_file(".purrdr/push-phone-a/pairing").as_deref(),
        Some(format!("purrdr-push 1\nendpoint={ENDPOINT}\nkey={KEY}\nmuted=w1-session\nmuted=w2-session\n").as_str())
    );
}

async fn update_muted_on_an_unpaired_device_is_rejected() {
    let harness = Harness::start().await;
    let before = harness.home_snapshot();
    let mut client = harness.client().await;

    let unpaired = client.request(update_muted("phone-a", json!(["w1-session"]))).await;
    let invalid_device = client.request(update_muted("../x", json!([]))).await;

    assert_eq!(unpaired["code"], "push_not_paired", "{unpaired}");
    assert_eq!(invalid_device["code"], "invalid_message", "{invalid_device}");
    assert_eq!(harness.home_snapshot(), before);
    assert!(harness.calls().is_empty());
}

async fn update_muted_rejects_multiline_sessions_and_read_only_devices() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client.request(install("phone-a")).await;
    let paired = harness.home_snapshot();
    let mut read_only = harness.read_only_client().await;

    let multiline = client.request(update_muted("phone-a", json!(["a\nkey=evil"]))).await;
    let refused = read_only.request(update_muted("phone-a", json!(["w1-session"]))).await;

    assert_eq!(multiline["code"], "invalid_message", "{multiline}");
    assert_eq!(refused["code"], "read_only_device", "{refused}");
    assert_eq!(harness.home_snapshot(), paired);
}

// MARK: - 測試通知(push.test,#60)

const OTHER_KEY: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

fn push_test(device_id: &str) -> Value {
    json!({"type": "push.test", "request_id": "t1", "device_id": device_id})
}

async fn push_test_sends_one_test_event_to_this_device_only() {
    let harness = Harness::start().await;
    let backend = FakePushBackend::start();
    harness.write_pairing("phone-a", &backend.endpoint(), KEY);
    harness.write_pairing("tablet-b", &backend.endpoint(), OTHER_KEY);
    let mut client = harness.client().await;

    let reply = client.request(push_test("phone-a")).await;

    assert_eq!(reply, json!({"type": "push.test.result", "request_id": "t1"}));
    assert_eq!(backend.bodies(), vec![json!({"key": KEY, "event": "test"})]);
    assert!(harness.calls().is_empty(), "不經過 herdr:{:?}", harness.calls());
}

async fn push_test_forwards_the_host_name_shown_in_the_app() {
    let harness = Harness::start().await;
    let backend = FakePushBackend::start();
    harness.write_pairing("phone-a", &backend.endpoint(), KEY);
    let mut client = harness.client().await;
    let mut named = push_test("phone-a");
    named["host_name"] = json!("Studio Mac");
    let mut multiline = push_test("phone-a");
    multiline["host_name"] = json!("Studio\nMac");
    let mut too_long = push_test("phone-a");
    too_long["host_name"] = json!("x".repeat(65));

    let reply = client.request(named).await;
    let rejected = [client.request(multiline).await, client.request(too_long).await];

    assert_eq!(reply["type"], "push.test.result", "{reply}");
    assert_eq!(backend.bodies(), vec![json!({"key": KEY, "event": "test", "host_name": "Studio Mac"})]);
    for rejected in rejected {
        assert_eq!(rejected["code"], "invalid_message", "{rejected}");
    }
}

async fn push_test_without_a_pairing_the_backend_knows_reports_not_paired() {
    let harness = Harness::start().await;
    let backend = FakePushBackend::start();
    let mut client = harness.client().await;

    let unpaired = client.request(push_test("phone-a")).await;
    harness.write_pairing("phone-a", &backend.endpoint(), KEY);
    backend.respond_to_key_with(KEY, 404, r#"{"error":{"code":"pairing_not_found","message":"No such pairing."}}"#);
    let unknown = client.request(push_test("phone-a")).await;
    let kept = harness.home_file(".purrdr/push-phone-a/pairing");
    backend.respond_to_key(KEY, 410);
    let revoked = client.request(push_test("phone-a")).await;

    assert_eq!(unpaired["code"], "push_not_paired", "{unpaired}");
    assert_eq!(unknown["code"], "push_not_paired", "{unknown}");
    assert!(kept.is_some(), "後台回 404 不刪本機配對");
    assert_eq!(revoked["code"], "push_not_paired", "{revoked}");
    assert!(!harness.home.path().join(".purrdr/push-phone-a").exists(), "後台回 410 刪掉這組配對");
}

async fn push_test_backend_failures_carry_the_backend_error_code() {
    let harness = Harness::start().await;
    let backend = FakePushBackend::start();
    harness.write_pairing("phone-a", &backend.endpoint(), KEY);
    let mut client = harness.client().await;

    backend.respond_to_key_with(KEY, 403, r#"{"error":{"code":"subscription_required","message":"Needs Plus."}}"#);
    let subscription = client.request(push_test("phone-a")).await;
    backend.respond_to_key_with(KEY, 500, "oops");
    let server_error = client.request(push_test("phone-a")).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    harness.write_pairing("phone-a", &format!("http://{closed}/v1/push"), KEY);
    let unreachable = client.request(push_test("phone-a")).await;

    assert_eq!(subscription["code"], "push_send_failed", "{subscription}");
    assert_eq!(subscription["message"], "subscription_required");
    assert_eq!(server_error["code"], "push_send_failed", "{server_error}");
    assert_eq!(server_error["message"], "http 500");
    assert_eq!(unreachable["code"], "push_send_failed", "{unreachable}");
    for reply in [&subscription, &server_error, &unreachable] {
        assert!(!reply.to_string().contains(KEY), "{reply}");
    }
    assert!(harness.home_file(".purrdr/push-phone-a/pairing").is_some(), "送不出去不刪配對");
}

async fn push_test_does_not_follow_redirects_with_the_key() {
    let harness = Harness::start().await;
    let backend = FakePushBackend::start();
    harness.write_pairing("phone-a", &backend.endpoint(), KEY);
    backend.respond_to_key_with(KEY, 307, "");
    let mut client = harness.client().await;

    let reply = client.request(push_test("phone-a")).await;

    assert_eq!(reply["code"], "push_send_failed", "{reply}");
    assert_eq!(reply["message"], "http 307");
    assert_eq!(backend.bodies().len(), 1, "沒有把密鑰再送到轉址的目的地");
}

async fn push_test_is_refused_for_read_only_devices_and_audited_without_the_key() {
    let harness = Harness::start().await;
    let backend = FakePushBackend::start();
    harness.write_pairing("phone-a", &backend.endpoint(), KEY);
    let mut read_only = harness.read_only_client().await;
    let mut client = harness.client().await;

    let refused = read_only.request(push_test("phone-a")).await;
    client.request(push_test("phone-a")).await;

    assert_eq!(refused["code"], "read_only_device", "{refused}");
    assert_eq!(backend.bodies().len(), 1, "唯讀裝置的請求沒有送到後台");
    let audit = harness.service.audit_log();
    assert!(audit.contains("push.test"), "{audit}");
    assert!(!audit.contains(KEY), "{audit}");
}

// MARK: - 自動更新外掛(#76)

/// 假 herdr 的資料夾:直接以 `fake-herdr <資料夾>` 當 herdr,不經主機服務。
struct UpdateSetup {
    fake: TempDir,
    herdr: HerdrCli,
}

impl UpdateSetup {
    fn new() -> UpdateSetup {
        let fake = tempfile::tempdir().unwrap();
        let herdr = HerdrCli::at_path(
            std::env::current_exe().unwrap(),
            vec![FAKE_HERDR.to_owned(), fake.path().to_string_lossy().into_owned()],
        );
        UpdateSetup { fake, herdr }
    }

    fn mark(&self, name: &str) {
        std::fs::write(self.fake.path().join(name), b"").unwrap();
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.fake.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

async fn updating_an_installed_plugin_runs_herdr_plugin_update() {
    let setup = UpdateSetup::new();
    setup.mark("installed");

    let result = PluginUpdater::standalone(setup.herdr.clone()).update().await;

    assert_eq!(result, Ok(PluginUpdate::Updated));
    assert!(setup.calls().contains(&"plugin update purrdr.push --yes".to_owned()), "{:?}", setup.calls());
}

async fn updating_without_the_plugin_does_not_run_plugin_update() {
    let setup = UpdateSetup::new();

    let result = PluginUpdater::standalone(setup.herdr.clone()).update().await;

    assert_eq!(result, Ok(PluginUpdate::NotInstalled));
    assert!(
        !setup
            .calls()
            .iter()
            .any(|call| call.contains("plugin update") || call.contains("plugin install")),
        "沒裝外掛不更新也不代裝:{:?}",
        setup.calls()
    );
}

async fn a_failing_plugin_update_is_reported_with_herdrs_message() {
    let setup = UpdateSetup::new();
    setup.mark("installed");
    setup.mark("update-fails");

    let result = PluginUpdater::standalone(setup.herdr.clone()).update().await;

    assert_eq!(result, Err("git pull failed".to_owned()));
}

async fn failing_to_list_plugins_is_a_failure_and_does_not_update() {
    let setup = UpdateSetup::new();
    setup.mark("installed");
    setup.mark("list-fails");

    let result = PluginUpdater::standalone(setup.herdr.clone()).update().await;

    assert_eq!(result, Err("herdr server is not running".to_owned()));
    assert!(!setup.calls().iter().any(|call| call.contains("plugin update")), "{:?}", setup.calls());
}

// MARK: - 執行

type TestCase = (&'static str, fn() -> Pin<Box<dyn Future<Output = ()>>>);

fn tests() -> Vec<TestCase> {
    vec![
        ("status_of_a_fresh_host_reports_nothing_installed", || {
            Box::pin(status_of_a_fresh_host_reports_nothing_installed())
        }),
        ("install_writes_the_pairing_and_installs_the_plugin_without_session", || {
            Box::pin(install_writes_the_pairing_and_installs_the_plugin_without_session())
        }),
        ("second_device_only_adds_its_pairing", || Box::pin(second_device_only_adds_its_pairing())),
        ("failed_plugin_install_leaves_the_home_as_before", || {
            Box::pin(failed_plugin_install_leaves_the_home_as_before())
        }),
        ("herdr_failing_to_list_plugins_is_reported", || {
            Box::pin(herdr_failing_to_list_plugins_is_reported())
        }),
        ("removing_one_device_keeps_the_plugin_for_the_others", || {
            Box::pin(removing_one_device_keeps_the_plugin_for_the_others())
        }),
        ("removing_the_last_device_uninstalls_the_plugin_and_restores_the_home", || {
            Box::pin(removing_the_last_device_uninstalls_the_plugin_and_restores_the_home())
        }),
        ("plugin_the_user_installed_is_never_uninstalled", || {
            Box::pin(plugin_the_user_installed_is_never_uninstalled())
        }),
        ("read_only_device_can_query_but_not_install_or_remove", || {
            Box::pin(read_only_device_can_query_but_not_install_or_remove())
        }),
        ("unsafe_device_ids_and_pairings_are_rejected_without_touching_the_host", || {
            Box::pin(unsafe_device_ids_and_pairings_are_rejected_without_touching_the_host())
        }),
        ("failed_reinstall_restores_the_previous_pairing", || {
            Box::pin(failed_reinstall_restores_the_previous_pairing())
        }),
        ("reinstall_overwrites_the_key_without_reinstalling_the_plugin", || {
            Box::pin(reinstall_overwrites_the_key_without_reinstalling_the_plugin())
        }),
        ("updating_an_installed_plugin_runs_herdr_plugin_update", || {
            Box::pin(updating_an_installed_plugin_runs_herdr_plugin_update())
        }),
        ("updating_without_the_plugin_does_not_run_plugin_update", || {
            Box::pin(updating_without_the_plugin_does_not_run_plugin_update())
        }),
        ("a_failing_plugin_update_is_reported_with_herdrs_message", || {
            Box::pin(a_failing_plugin_update_is_reported_with_herdrs_message())
        }),
        ("failing_to_list_plugins_is_a_failure_and_does_not_update", || {
            Box::pin(failing_to_list_plugins_is_a_failure_and_does_not_update())
        }),
        ("update_muted_rewrites_only_muted_without_herdr", || {
            Box::pin(update_muted_rewrites_only_muted_without_herdr())
        }),
        ("update_muted_keeps_key_and_endpoint_and_sorts_sessions", || {
            Box::pin(update_muted_keeps_key_and_endpoint_and_sorts_sessions())
        }),
        ("update_muted_on_an_unpaired_device_is_rejected", || {
            Box::pin(update_muted_on_an_unpaired_device_is_rejected())
        }),
        ("update_muted_rejects_multiline_sessions_and_read_only_devices", || {
            Box::pin(update_muted_rejects_multiline_sessions_and_read_only_devices())
        }),
        ("install_and_remove_are_audited_without_the_key", || {
            Box::pin(install_and_remove_are_audited_without_the_key())
        }),
        ("push_test_sends_one_test_event_to_this_device_only", || {
            Box::pin(push_test_sends_one_test_event_to_this_device_only())
        }),
        ("push_test_forwards_the_host_name_shown_in_the_app", || {
            Box::pin(push_test_forwards_the_host_name_shown_in_the_app())
        }),
        ("push_test_without_a_pairing_the_backend_knows_reports_not_paired", || {
            Box::pin(push_test_without_a_pairing_the_backend_knows_reports_not_paired())
        }),
        ("push_test_backend_failures_carry_the_backend_error_code", || {
            Box::pin(push_test_backend_failures_carry_the_backend_error_code())
        }),
        ("push_test_does_not_follow_redirects_with_the_key", || {
            Box::pin(push_test_does_not_follow_redirects_with_the_key())
        }),
        ("push_test_is_refused_for_read_only_devices_and_audited_without_the_key", || {
            Box::pin(push_test_is_refused_for_read_only_devices_and_audited_without_the_key())
        }),
    ]
}

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some(FAKE_HERDR) {
        return std::process::ExitCode::from(fake_herdr(&arguments[1..]) as u8);
    }
    // cargo test 傳進來的篩選字(不是 `--` 開頭的參數)。
    let filters: Vec<&String> = arguments.iter().filter(|argument| !argument.starts_with('-')).collect();
    let selected: Vec<TestCase> = tests()
        .into_iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|filter| name.contains(filter.as_str())))
        .collect();
    println!("\nrunning {} tests", selected.len());
    let mut failures = Vec::new();
    for (name, test) in &selected {
        let outcome = std::panic::catch_unwind(|| {
            tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(test());
        });
        println!("test {name} ... {}", if outcome.is_ok() { "ok" } else { "FAILED" });
        if outcome.is_err() {
            failures.push(*name);
        }
    }
    println!("\ntest result: {} passed; {} failed", selected.len() - failures.len(), failures.len());
    if failures.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        println!("failures: {failures:?}");
        std::process::ExitCode::FAILURE
    }
}
