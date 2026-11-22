//! Claude 方案用量(raychiutw/purrdr.host#62、#64):
//! - 安裝與還原 status line 包裝:`claude_usage.status`／`install`／`remove`,與 app 經 SSH 的
//!   `ClaudePlanUsageHost`／`ReversibleHostWrite` 共用 `~/.purrdr/claude-usage/journal.json`,兩條路徑可互相關閉;
//! - 讀用量:`usage.claude`(整台主機)與 `usage.session_context`(每個 session)。
//!
//! 以真正的 WebSocket client 從協定這一端驗證，外部行為只看回應與暫存家目錄裡的檔案(絕不碰真正的家目錄)。

mod support;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use purrdr_host_service::transcript::TranscriptEnvironment;
use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, TestHostService, recorded, start_service_with_transcripts};
use tempfile::TempDir;

const WRAPPER_SH: &str = include_str!("../../../claude-usage/statusline.sh");
const WRAPPER_PS1: &str = include_str!("../../../claude-usage/statusline.ps1");

/// 這個平台的主機服務寫進 `statusLine.command` 的指令:Windows 是 PowerShell 版(家目錄的反斜線換成 `/`),其他是 `.sh` 版。
fn expected_wrapper_command(home: &std::path::Path) -> String {
    if cfg!(windows) {
        let home = home.to_string_lossy().replace('\\', "/");
        format!("powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '{home}/.purrdr/claude-usage/statusline.ps1'")
    } else {
        "~/.purrdr/claude-usage/statusline.sh".to_owned()
    }
}

/// 這個平台安裝的包裝腳本檔名(相對家目錄)與內容。
const WRAPPER_FILE: &str = if cfg!(windows) {
    ".purrdr/claude-usage/statusline.ps1"
} else {
    ".purrdr/claude-usage/statusline.sh"
};
const WRAPPER_SCRIPT: &str = if cfg!(windows) { WRAPPER_PS1 } else { WRAPPER_SH };

// MARK: - 測試環境

struct Harness {
    _herdr: FakeHerdr,
    home: TempDir,
    service: TestHostService,
}

impl Harness {
    /// 家目錄已有 `.claude/`(裝了 Claude Code)。
    async fn start() -> Harness {
        let harness = Harness::start_bare(None).await;
        std::fs::create_dir_all(harness.home.path().join(".claude")).unwrap();
        harness
    }

    /// 家目錄是空的;`claude_config_dir` 是 `CLAUDE_CONFIG_DIR`(相對家目錄)。
    async fn start_bare(claude_config_dir: Option<&str>) -> Harness {
        let herdr = FakeHerdr::start(vec![("ping", recorded("ping.ndjson"))]).await;
        let home = tempfile::tempdir().unwrap();
        let transcripts = TranscriptEnvironment {
            home: Some(home.path().to_owned()),
            claude_config_dir: claude_config_dir.map(|directory| home.path().join(directory)),
            ..TranscriptEnvironment::default()
        };
        let service = start_service_with_transcripts(herdr.socket.clone(), transcripts).await;
        Harness { _herdr: herdr, home, service }
    }

    async fn client(&self) -> TestClient {
        let mut client = TestClient::connect(&self.service).await;
        client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;
        client
    }

    async fn send(&self, message: Value) -> Value {
        self.client().await.request(message).await
    }

    async fn send_read_only(&self, message: Value) -> Value {
        let device = self.service.pair_device("借出的 iPad", true);
        let mut client = TestClient::connect_with_token(&self.service, Some(&device.device_token)).await;
        client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;
        client.request(message).await
    }

    async fn install(&self) -> Value {
        self.send(json!({"type": "claude_usage.install", "request_id": "i1"})).await
    }

    async fn remove(&self) -> Value {
        self.send(json!({"type": "claude_usage.remove", "request_id": "x1"})).await
    }

    async fn status(&self) -> Value {
        self.send(json!({"type": "claude_usage.status", "request_id": "s1"})).await
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.home.path().join(relative)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.path(relative)).ok()
    }

    fn settings(&self) -> Value {
        serde_json::from_str(&self.read(".claude/settings.json").expect("settings.json 存在")).unwrap()
    }

    fn journal(&self) -> Value {
        serde_json::from_str(&self.read(".purrdr/claude-usage/journal.json").expect("journal.json 存在")).unwrap()
    }

    fn exists(&self, relative: &str) -> bool {
        self.path(relative).exists()
    }

    fn wrapper_command(&self) -> String {
        expected_wrapper_command(self.home.path())
    }

    /// app(Foundation `JSONSerialization`,prettyPrinted + sortedKeys)寫的設定:鍵與值之間是 ` : `。
    fn app_installed_settings(&self, extra_before: &str) -> String {
        let command = self.wrapper_command();
        format!("{{\n{extra_before}  \"statusLine\" : {{\n    \"command\" : \"{command}\",\n    \"type\" : \"command\"\n  }}\n}}\n")
    }
}

fn state(kind: &str, request_id: &str, installed: bool, wrapper_active: bool, claude_found: bool) -> Value {
    json!({"type": kind, "request_id": request_id, "installed": installed, "wrapper_active": wrapper_active, "claude_found": claude_found})
}

fn encoded(text: &str) -> String {
    STANDARD.encode(text)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn rfc3339(seconds: u64) -> String {
    humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(seconds)).to_string()
}

/// app(Swift `JSONEncoder`,sortedKeys)寫的安裝紀錄:`/` 會跳脫成 `\/`,所以路徑與 base64 裡的 `/` 都是 `\/`。
fn app_journal(original: Option<&str>, installed: &str) -> String {
    let mut entry = json!({"path": ".claude/settings.json", "installed": encoded(installed)});
    if let Some(original) = original {
        entry["original"] = json!(encoded(original));
    }
    json!({"version": 1, "entries": [entry]}).to_string().replace('/', "\\/")
}

// MARK: - 安裝

#[tokio::test]
async fn install_on_settings_without_status_line_keeps_other_settings() {
    let harness = Harness::start().await;
    let original = "{\n    \"theme\": \"dark\",\n    \"permissions\": {\"allow\": [\"Bash(ls:*)\"]}\n}\n";
    harness.write(".claude/settings.json", original);

    let reply = harness.install().await;

    assert_eq!(reply, state("claude_usage.install.result", "i1", true, true, true), "{reply}");
    let settings = harness.settings();
    assert_eq!(settings["theme"], "dark");
    assert_eq!(settings["permissions"], json!({"allow": ["Bash(ls:*)"]}));
    assert_eq!(settings["statusLine"], json!({"type": "command", "command": harness.wrapper_command()}));
    assert_eq!(harness.read(WRAPPER_FILE).as_deref(), Some(WRAPPER_SCRIPT));
    assert!(!harness.exists(".purrdr/claude-usage/original-command"), "原本沒有 status line,沒有東西要存");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(harness.path(WRAPPER_FILE)).unwrap().permissions().mode();
        assert_ne!(mode & 0o100, 0, "包裝腳本要能執行:{mode:o}");
    }
    // 安裝紀錄是 app 的 HostWriteJournal:version、entries(path／original／installed 為 base64)。
    let journal = harness.journal();
    assert_eq!(journal["version"], 1);
    assert!(journal.get("createdDirectories").is_none(), "{journal}");
    assert_eq!(journal["entries"].as_array().unwrap().len(), 1);
    let entry = &journal["entries"][0];
    assert_eq!(entry["path"], ".claude/settings.json");
    assert_eq!(entry["original"], encoded(original));
    assert_eq!(entry["installed"], encoded(&harness.read(".claude/settings.json").unwrap()));
}

#[tokio::test]
async fn install_writes_settings_the_way_the_app_does() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{}");

    harness.install().await;

    // 與 app(JSONSerialization prettyPrinted + sortedKeys + 結尾換行)同樣的排版:app 不必因排版不同而重寫。
    assert_eq!(harness.read(".claude/settings.json").unwrap(), harness.app_installed_settings(""));
}

#[tokio::test]
async fn install_stores_the_existing_status_line_command_as_original() {
    let harness = Harness::start().await;
    let command = "~/bin/my-line.sh --compact";
    harness.write(
        ".claude/settings.json",
        &json!({"statusLine": {"type": "command", "command": command, "padding": 2}, "model": "opus"}).to_string(),
    );

    harness.install().await;

    assert_eq!(harness.read(".purrdr/claude-usage/original-command").as_deref(), Some(command));
    let settings = harness.settings();
    assert_eq!(
        settings["statusLine"],
        json!({"type": "command", "command": harness.wrapper_command(), "padding": 2})
    );
    assert_eq!(settings["model"], "opus");
}

#[tokio::test]
async fn install_without_settings_file_creates_it() {
    let harness = Harness::start().await;

    let reply = harness.install().await;

    assert_eq!(reply["type"], "claude_usage.install.result", "{reply}");
    assert_eq!(
        harness.settings(),
        json!({"statusLine": {"type": "command", "command": harness.wrapper_command()}})
    );
    assert!(harness.journal()["entries"][0].get("original").is_none(), "原本不存在:紀錄沒有 original");

    // 還原:原本不存在就刪除,家目錄回到安裝前(.claude 是使用者的,保留)。
    let removed = harness.remove().await;
    assert_eq!(removed, state("claude_usage.remove.result", "x1", false, false, true), "{removed}");
    assert!(!harness.exists(".claude/settings.json"));
    assert!(!harness.exists(".purrdr/claude-usage"));
    assert!(harness.exists(".claude"));
}

#[tokio::test]
async fn install_without_claude_settings_directory_is_refused() {
    let harness = Harness::start_bare(None).await;

    let reply = harness.install().await;

    assert_eq!(reply["type"], "error", "{reply}");
    assert_eq!(reply["code"], "claude_not_found");
    assert!(!harness.exists(".purrdr"), "什麼都不寫");
    assert!(!harness.exists(".claude"));
}

#[tokio::test]
async fn reinstall_keeps_the_first_original_and_never_stores_the_wrapper() {
    let harness = Harness::start().await;
    let first = "{\"statusLine\":{\"type\":\"command\",\"command\":\"echo first\"}}";
    harness.write(".claude/settings.json", first);
    harness.install().await;
    let journal_before = harness.journal();

    let reply = harness.install().await;

    assert_eq!(reply["type"], "claude_usage.install.result", "{reply}");
    assert_eq!(harness.read(".purrdr/claude-usage/original-command").as_deref(), Some("echo first"));
    assert_eq!(harness.journal()["entries"][0]["original"], journal_before["entries"][0]["original"]);
    assert_eq!(harness.journal()["entries"].as_array().unwrap().len(), 1, "同一個檔案只有一筆");
    assert_eq!(harness.settings()["statusLine"]["command"], harness.wrapper_command());
    // 還原後回到第一次安裝前。
    harness.remove().await;
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some(first));
}

/// 設定裡已經是包裝(例如安裝紀錄被刪掉)時重裝:包裝不能被存成「原本的」,否則執行時會一直呼叫自己。
#[tokio::test]
async fn install_over_an_existing_wrapper_without_journal_does_not_store_it_as_original() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", &harness.app_installed_settings(""));

    let reply = harness.install().await;

    assert_eq!(reply["type"], "claude_usage.install.result", "{reply}");
    assert!(!harness.exists(".purrdr/claude-usage/original-command"));
}

#[tokio::test]
async fn reinstall_after_user_changed_other_settings_keeps_them_and_the_original() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{\"statusLine\":{\"type\":\"command\",\"command\":\"echo first\"}}");
    harness.install().await;
    let mut settings = harness.settings();
    settings["model"] = json!("sonnet");
    harness.write(".claude/settings.json", &settings.to_string());

    harness.install().await;

    assert_eq!(harness.settings()["model"], "sonnet");
    assert_eq!(harness.read(".purrdr/claude-usage/original-command").as_deref(), Some("echo first"));
    assert_eq!(
        harness.journal()["entries"][0]["original"],
        encoded("{\"statusLine\":{\"type\":\"command\",\"command\":\"echo first\"}}")
    );
}

#[tokio::test]
async fn install_with_invalid_settings_json_is_refused_and_leaves_everything_alone() {
    for broken in [
        "{\"statusLine\": ",
        "{\n  // 註解\n  \"theme\": \"dark\"\n}",
        "{\"a\": 1,}",
        "[1, 2]",
        "\"text\"",
        "",
    ] {
        let harness = Harness::start().await;
        harness.write(".claude/settings.json", broken);

        let reply = harness.install().await;

        assert_eq!(reply["type"], "error", "{broken:?}: {reply}");
        assert_eq!(reply["code"], "claude_settings_not_json", "{broken:?}: {reply}");
        assert_eq!(harness.read(".claude/settings.json").as_deref(), Some(broken), "設定檔一個 byte 都不動");
        assert!(!harness.exists(".purrdr"), "{broken:?}:連包裝腳本都不寫");
    }
}

#[tokio::test]
async fn install_refuses_settings_that_are_not_a_regular_file() {
    let harness = Harness::start().await;
    std::fs::create_dir_all(harness.path(".claude/settings.json")).unwrap();

    let reply = harness.install().await;

    assert_eq!(reply["code"], "host_write_failed", "{reply}");
    assert!(!harness.exists(".purrdr"));
}

#[tokio::test]
async fn read_only_device_cannot_install_or_remove_but_can_read_status() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{}");

    let install = harness.send_read_only(json!({"type": "claude_usage.install", "request_id": "i2"})).await;
    let remove = harness.send_read_only(json!({"type": "claude_usage.remove", "request_id": "x2"})).await;
    let status = harness.send_read_only(json!({"type": "claude_usage.status", "request_id": "s2"})).await;

    assert_eq!(install["code"], "read_only_device", "{install}");
    assert_eq!(remove["code"], "read_only_device", "{remove}");
    assert_eq!(status, state("claude_usage.status.result", "s2", false, false, true), "{status}");
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some("{}"));
    assert!(!harness.exists(".purrdr"));
}

#[tokio::test]
async fn install_ignores_claude_config_dir_and_always_uses_the_home_settings_like_the_app() {
    let harness = Harness::start_bare(Some("elsewhere/claude")).await;
    harness.write("elsewhere/claude/settings.json", "{\"theme\":\"light\"}");
    harness.write(".claude/settings.json", "{\"theme\":\"dark\"}");

    let reply = harness.install().await;

    assert_eq!(reply["type"], "claude_usage.install.result", "{reply}");
    assert_eq!(
        harness.read("elsewhere/claude/settings.json").as_deref(),
        Some("{\"theme\":\"light\"}"),
        "CLAUDE_CONFIG_DIR 不動"
    );
    assert_eq!(harness.settings()["statusLine"]["command"], harness.wrapper_command());
    assert_eq!(harness.journal()["entries"][0]["path"], ".claude/settings.json");
    harness.remove().await;
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some("{\"theme\":\"dark\"}"));
}

#[tokio::test]
async fn reinstall_after_the_user_replaced_the_status_line_takes_the_new_one_as_original() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{\"statusLine\":{\"type\":\"command\",\"command\":\"echo first\"}}");
    harness.install().await;
    harness.write(".claude/settings.json", "{\"statusLine\":{\"type\":\"command\",\"command\":\"echo mine\"}}");

    let reinstall = harness.install().await;

    assert_eq!(reinstall["type"], "claude_usage.install.result", "{reinstall}");
    assert_eq!(harness.read(".purrdr/claude-usage/original-command").as_deref(), Some("echo mine"));
    harness.remove().await;
    assert_eq!(
        harness.settings()["statusLine"]["command"],
        "echo mine",
        "手動改的 status line 不會在還原時消失"
    );
}

// MARK: - 狀態

#[tokio::test]
async fn status_follows_install_and_user_replacing_the_status_line() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{}");
    assert_eq!(harness.status().await, state("claude_usage.status.result", "s1", false, false, true));

    harness.install().await;
    assert_eq!(harness.status().await, state("claude_usage.status.result", "s1", true, true, true));

    harness.write(".claude/settings.json", "{\"statusLine\":{\"type\":\"command\",\"command\":\"echo mine\"}}");
    assert_eq!(harness.status().await, state("claude_usage.status.result", "s1", true, false, true));

    harness.remove().await;
    assert_eq!(harness.status().await, state("claude_usage.status.result", "s1", false, false, true));
}

#[tokio::test]
async fn status_without_claude_directory_reports_not_found() {
    let harness = Harness::start_bare(None).await;

    assert_eq!(harness.status().await, state("claude_usage.status.result", "s1", false, false, false));
}

#[tokio::test]
async fn unreadable_journal_is_reported_and_never_overwritten() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{}");
    harness.write(".purrdr/claude-usage/journal.json", "not json");

    let status = harness.status().await;
    let install = harness.install().await;
    let remove = harness.remove().await;

    for reply in [status, install, remove] {
        assert_eq!(reply["code"], "host_write_failed", "{reply}");
    }
    assert_eq!(harness.read(".purrdr/claude-usage/journal.json").as_deref(), Some("not json"));
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some("{}"));
}

// MARK: - 還原

#[tokio::test]
async fn remove_restores_byte_identical_settings_when_unchanged() {
    let harness = Harness::start().await;
    // 刻意不是 app／主機服務會產生的排版:四格縮排、CRLF、沒有結尾換行、非 ASCII。
    let original = "{\r\n    \"statusLine\": {\"type\": \"command\", \"command\": \"echo 貓\"},\r\n    \"theme\":   \"dark\"\r\n}";
    harness.write(".claude/settings.json", original);
    harness.install().await;
    assert_ne!(harness.read(".claude/settings.json").as_deref(), Some(original));

    let reply = harness.remove().await;

    assert_eq!(reply, state("claude_usage.remove.result", "x1", false, false, true), "{reply}");
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some(original), "與安裝前完全相同");
    assert!(!harness.exists(".purrdr/claude-usage"), "包裝資料夾移除");
}

#[tokio::test]
async fn remove_after_user_edited_settings_only_undoes_the_status_line() {
    let harness = Harness::start().await;
    harness.write(
        ".claude/settings.json",
        &json!({"statusLine": {"type": "command", "command": "echo mine", "padding": 1}, "theme": "dark"}).to_string(),
    );
    harness.install().await;
    let mut settings = harness.settings();
    settings["model"] = json!("opus");
    harness.write(".claude/settings.json", &settings.to_string());

    let reply = harness.remove().await;

    assert_eq!(reply["type"], "claude_usage.remove.result", "{reply}");
    assert_eq!(
        harness.settings(),
        json!({"statusLine": {"type": "command", "command": "echo mine", "padding": 1}, "theme": "dark", "model": "opus"}),
        "statusLine 換回原本的,使用者後來加的 model 保留"
    );
    assert!(!harness.exists(".purrdr/claude-usage"));
}

#[tokio::test]
async fn remove_after_user_edit_drops_status_line_when_there_was_none_before() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{\"theme\":\"dark\"}");
    harness.install().await;
    let mut settings = harness.settings();
    settings["model"] = json!("opus");
    harness.write(".claude/settings.json", &settings.to_string());

    harness.remove().await;

    assert_eq!(harness.settings(), json!({"theme": "dark", "model": "opus"}));
}

#[tokio::test]
async fn remove_leaves_a_status_line_the_user_replaced_after_install() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{\"statusLine\":{\"type\":\"command\",\"command\":\"echo first\"}}");
    harness.install().await;
    let replaced = json!({"statusLine": {"type": "command", "command": "echo second"}, "theme": "light"}).to_string();
    harness.write(".claude/settings.json", &replaced);

    harness.remove().await;

    assert_eq!(
        harness.read(".claude/settings.json").as_deref(),
        Some(replaced.as_str()),
        "不動使用者自己換的 status line"
    );
    assert!(!harness.exists(".purrdr/claude-usage"));
}

#[tokio::test]
async fn remove_respects_a_settings_file_the_user_deleted_after_install() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{\"theme\":\"dark\"}");
    harness.install().await;
    std::fs::remove_file(harness.path(".claude/settings.json")).unwrap();

    let reply = harness.remove().await;

    assert_eq!(reply["type"], "claude_usage.remove.result", "{reply}");
    assert!(!harness.exists(".claude/settings.json"), "使用者刪掉的不再建立");
    assert!(!harness.exists(".purrdr/claude-usage"));
}

#[tokio::test]
async fn remove_with_edited_invalid_settings_is_refused_and_keeps_everything() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{\"theme\":\"dark\"}");
    harness.install().await;
    harness.write(".claude/settings.json", "{ broken");

    let reply = harness.remove().await;

    assert_eq!(reply["code"], "claude_settings_not_json", "{reply}");
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some("{ broken"));
    assert!(harness.exists(".purrdr/claude-usage/journal.json"), "沒還原成功就保留紀錄,之後還能還原");
}

#[tokio::test]
async fn remove_without_a_journal_does_nothing() {
    let harness = Harness::start().await;
    harness.write(".claude/settings.json", "{\"theme\":\"dark\"}");
    harness.write(".purrdr/claude-usage/usage.json", "{}");

    let reply = harness.remove().await;

    assert_eq!(reply, state("claude_usage.remove.result", "x1", false, false, true), "{reply}");
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some("{\"theme\":\"dark\"}"));
    assert!(harness.exists(".purrdr/claude-usage/usage.json"));
}

// MARK: - 與 app 的安裝紀錄互通

#[tokio::test]
async fn remove_restores_an_install_made_by_the_app_over_ssh() {
    let harness = Harness::start().await;
    let original = "{\n  \"statusLine\": { \"type\": \"command\", \"command\": \"~/old/line.sh\" },\n  \"theme\": \"dark\"\n}\n";
    // app 裝出來的設定、紀錄、包裝與原本的指令。
    let installed = harness.app_installed_settings("  \"theme\" : \"dark\",\n");
    harness.write(".claude/settings.json", &installed);
    harness.write(".purrdr/claude-usage/journal.json", &app_journal(Some(original), &installed));
    harness.write(WRAPPER_FILE, "#!/bin/sh\n");
    harness.write(".purrdr/claude-usage/original-command", "~/old/line.sh");
    harness.write(".purrdr/claude-usage/usage.json", "{}");
    assert_eq!(harness.status().await, state("claude_usage.status.result", "s1", true, true, true));

    let reply = harness.remove().await;

    assert_eq!(reply, state("claude_usage.remove.result", "x1", false, false, true), "{reply}");
    assert_eq!(harness.read(".claude/settings.json").as_deref(), Some(original), "byte 相同地換回");
    assert!(!harness.exists(".purrdr/claude-usage"));
}

#[tokio::test]
async fn remove_merges_an_app_install_the_user_edited_afterwards() {
    let harness = Harness::start().await;
    let original = "{\"statusLine\":{\"type\":\"command\",\"command\":\"~/old/line.sh\"}}";
    let installed = harness.app_installed_settings("");
    harness.write(".claude/settings.json", &harness.app_installed_settings("  \"model\" : \"opus\",\n"));
    harness.write(".purrdr/claude-usage/journal.json", &app_journal(Some(original), &installed));

    harness.remove().await;

    assert_eq!(
        harness.settings(),
        json!({"statusLine": {"type": "command", "command": "~/old/line.sh"}, "model": "opus"})
    );
    assert!(!harness.exists(".purrdr/claude-usage"));
}

#[tokio::test]
async fn remove_deletes_settings_the_app_created_and_nothing_else() {
    let harness = Harness::start().await;
    let installed = harness.app_installed_settings("");
    harness.write(".claude/settings.json", &installed);
    harness.write(".purrdr/claude-usage/journal.json", &app_journal(None, &installed));

    harness.remove().await;

    assert!(!harness.exists(".claude/settings.json"));
    assert!(!harness.exists(".purrdr/claude-usage"));
}

#[tokio::test]
async fn remove_ignores_unknown_journal_fields_and_removes_created_directories_when_empty() {
    let harness = Harness::start_bare(None).await;
    let installed = harness.app_installed_settings("");
    let journal = json!({
        "version": 1,
        "entries": [{"path": ".claude/settings.json", "installed": encoded(&installed)}],
        "createdDirectories": [".claude"],
        "somethingNew": true,
    })
    .to_string();
    harness.write(".claude/settings.json", &installed);
    harness.write(".purrdr/claude-usage/journal.json", &journal);

    harness.remove().await;

    assert!(!harness.exists(".claude"), "安裝時建立、現在是空的資料夾移除");
    assert!(!harness.exists(".purrdr/claude-usage"));
}

#[tokio::test]
async fn remove_refuses_journal_paths_outside_the_settings_file() {
    let harness = Harness::start().await;
    harness.write("victim.txt", "keep me");
    // 家目錄底下但不是 settings.json、跳出家目錄、絕對路徑:都不能被一份被竄改的紀錄拿來刪檔或覆寫。
    for path in ["../victim.txt", "victim.txt", ".claude/../victim.txt", "/etc/hosts"] {
        let journal = json!({"version": 1, "entries": [{"path": path, "installed": encoded("keep me")}]}).to_string();
        harness.write(".purrdr/claude-usage/journal.json", &journal);

        let reply = harness.remove().await;

        assert_eq!(reply["code"], "host_write_failed", "{path}: {reply}");
        assert_eq!(harness.read("victim.txt").as_deref(), Some("keep me"), "{path}");
        assert!(harness.exists(".purrdr/claude-usage/journal.json"), "{path}");
    }
}

// MARK: - 方案用量

fn usage_file(captured_at: u64, status_line_input: Value) -> String {
    json!({"captured_at": captured_at, "status_line_input": status_line_input}).to_string()
}

#[tokio::test]
async fn usage_without_install_is_empty_and_marked_not_installed() {
    let harness = Harness::start_bare(None).await;

    let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

    assert_eq!(
        reply,
        json!({"type": "usage.claude.result", "request_id": "u1", "installed": false, "periods": []}),
        "{reply}"
    );
}

#[tokio::test]
async fn usage_after_install_but_before_any_report_is_installed_without_captured_at() {
    let harness = Harness::start().await;
    harness.install().await;

    let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

    assert_eq!(
        reply,
        json!({"type": "usage.claude.result", "request_id": "u1", "installed": true, "periods": []}),
        "{reply}"
    );
}

#[tokio::test]
async fn usage_reads_both_windows_in_order() {
    let harness = Harness::start().await;
    harness.install().await;
    let captured = now() - 30;
    let five_hour = now() + 3_600;
    let week = now() + 5 * 86_400;
    harness.write(
        ".purrdr/claude-usage/usage.json",
        &usage_file(
            captured,
            json!({"rate_limits": {
                "seven_day": {"used_percentage": 12, "resets_at": week},
                "five_hour": {"used_percentage": 42.5, "resets_at": five_hour},
            }, "context_window": {"used_percentage": 3}}),
        ),
    );

    let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

    assert_eq!(reply["type"], "usage.claude.result", "{reply}");
    assert_eq!(reply["installed"], true);
    assert_eq!(reply["captured_at"], rfc3339(captured));
    assert_eq!(
        reply["periods"],
        json!([
            {"kind": "five_hour", "used_percentage": 42.5, "resets_at": rfc3339(five_hour)},
            {"kind": "week", "used_percentage": 12.0, "resets_at": rfc3339(week)},
        ]),
        "{reply}"
    );
}

#[tokio::test]
async fn usage_tolerates_missing_rate_limits_nulls_and_garbage() {
    let future = now() + 600;
    let cases = [
        json!({"model": {"id": "x"}}),
        json!({"rate_limits": null}),
        json!({"rate_limits": {"five_hour": null, "seven_day": "oops"}}),
        json!({"rate_limits": {"five_hour": {"used_percentage": null, "resets_at": future}, "seven_day": {"resets_at": future}}}),
        json!({"rate_limits": {"five_hour": {"used_percentage": "42", "resets_at": future}}}),
    ];
    for input in cases {
        let harness = Harness::start().await;
        harness.install().await;
        harness.write(".purrdr/claude-usage/usage.json", &usage_file(now(), input.clone()));

        let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

        assert_eq!(reply["type"], "usage.claude.result", "{input}: {reply}");
        assert_eq!(reply["periods"], json!([]), "{input}: {reply}");
        assert!(reply["captured_at"].is_string(), "{input}: {reply}");
    }
}

#[tokio::test]
async fn usage_keeps_a_window_without_resets_at_and_clamps_the_percentage() {
    let harness = Harness::start().await;
    harness.install().await;
    harness.write(
        ".purrdr/claude-usage/usage.json",
        &usage_file(
            now(),
            json!({"rate_limits": {"five_hour": {"used_percentage": 140.0, "resets_at": null}, "seven_day": {"used_percentage": -4}}}),
        ),
    );

    let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

    assert_eq!(
        reply["periods"],
        json!([{"kind": "five_hour", "used_percentage": 100.0}, {"kind": "week", "used_percentage": 0.0}]),
        "{reply}"
    );
}

#[tokio::test]
async fn usage_omits_windows_whose_reset_time_has_passed() {
    let harness = Harness::start().await;
    harness.install().await;
    let week = now() + 86_400;
    harness.write(
        ".purrdr/claude-usage/usage.json",
        &usage_file(
            now() - 7_200,
            json!({"rate_limits": {"five_hour": {"used_percentage": 99, "resets_at": now() - 60}, "seven_day": {"used_percentage": 50, "resets_at": week}}}),
        ),
    );

    let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

    assert_eq!(
        reply["periods"],
        json!([{"kind": "week", "used_percentage": 50.0, "resets_at": rfc3339(week)}]),
        "{reply}"
    );
}

#[tokio::test]
async fn usage_with_an_absurdly_large_reset_time_does_not_panic() {
    let harness = Harness::start().await;
    harness.install().await;
    harness.write(
        ".purrdr/claude-usage/usage.json",
        r#"{"captured_at": 1e30, "status_line_input": {"rate_limits": {"five_hour": {"used_percentage": 10, "resets_at": 1e30}}}}"#,
    );

    let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

    assert_eq!(reply["type"], "usage.claude.result", "{reply}");
    assert!(reply.get("captured_at").is_none(), "{reply}");
    assert_eq!(reply["periods"][0]["used_percentage"], 10.0, "{reply}");
    assert!(reply["periods"][0].get("resets_at").is_none(), "{reply}");
}

#[tokio::test]
async fn usage_with_a_corrupt_file_is_empty_not_an_error() {
    let harness = Harness::start().await;
    harness.install().await;
    harness.write(".purrdr/claude-usage/usage.json", "{ half");

    let reply = harness.send(json!({"type": "usage.claude", "request_id": "u1"})).await;

    assert_eq!(
        reply,
        json!({"type": "usage.claude.result", "request_id": "u1", "installed": true, "periods": []}),
        "{reply}"
    );
}

#[tokio::test]
async fn read_only_device_can_read_usage_and_session_context() {
    let harness = Harness::start().await;
    harness.install().await;
    harness.write(
        ".purrdr/claude-usage/sessions/abc.json",
        &json!({"captured_at": now(), "session_id": "abc", "status_line_input": {"context_window": {"used_percentage": 7}}}).to_string(),
    );

    let usage = harness.send_read_only(json!({"type": "usage.claude", "request_id": "u1"})).await;
    let sessions = harness
        .send_read_only(json!({"type": "usage.session_context", "request_id": "c1", "session_ids": ["abc"]}))
        .await;

    assert_eq!(usage["type"], "usage.claude.result", "{usage}");
    assert_eq!(sessions["type"], "usage.session_context.result", "{sessions}");
    assert_eq!(sessions["sessions"][0]["used_percentage"], 7.0);
}

// MARK: - 每個 session 的 context

fn session_file(session_id: &str, captured_at: u64, context_window: Value) -> String {
    json!({"captured_at": captured_at, "session_id": session_id, "status_line_input": {"session_id": session_id, "context_window": context_window}}).to_string()
}

fn write_session(harness: &Harness, session_id: &str, captured_at: u64, context_window: Value) {
    harness.write(
        &format!(".purrdr/claude-usage/sessions/{session_id}.json"),
        &session_file(session_id, captured_at, context_window),
    );
}

#[tokio::test]
async fn session_context_returns_each_requested_session_in_request_order() {
    let harness = Harness::start().await;
    let first = now() - 100;
    let second = now() - 50;
    write_session(
        &harness,
        "5d1c9f0e-3b7a",
        first,
        json!({"used_percentage": 23.5, "context_window_size": 200_000}),
    );
    write_session(
        &harness,
        "other_session.v2",
        second,
        json!({"used_percentage": null, "context_window_size": 1_000_000}),
    );
    write_session(&harness, "not-asked", now(), json!({"used_percentage": 1}));

    let reply = harness
        .send(json!({"type": "usage.session_context", "request_id": "c1", "session_ids": ["other_session.v2", "missing", "5d1c9f0e-3b7a"]}))
        .await;

    assert_eq!(
        reply,
        json!({"type": "usage.session_context.result", "request_id": "c1", "sessions": [
            {"session_id": "other_session.v2", "captured_at": rfc3339(second), "context_window_size": 1_000_000},
            {"session_id": "5d1c9f0e-3b7a", "captured_at": rfc3339(first), "used_percentage": 23.5, "context_window_size": 200_000},
        ]}),
        "{reply}"
    );
}

#[tokio::test]
async fn session_context_requires_a_non_empty_list_of_at_most_50_ids() {
    let harness = Harness::start().await;
    write_session(&harness, "abc", now(), json!({"used_percentage": 1}));
    let fifty: Vec<String> = (0..50).map(|index| format!("s{index}")).collect();
    let fifty_one: Vec<String> = (0..51).map(|index| format!("s{index}")).collect();

    for (label, message) in [
        ("省略", json!({"type": "usage.session_context", "request_id": "c1"})),
        ("空陣列", json!({"type": "usage.session_context", "request_id": "c1", "session_ids": []})),
        ("51 個", json!({"type": "usage.session_context", "request_id": "c1", "session_ids": fifty_one})),
    ] {
        let reply = harness.send(message).await;
        assert_eq!(reply["type"], "error", "{label}: {reply}");
        assert_eq!(reply["code"], "invalid_message", "{label}: {reply}");
    }
    let reply = harness
        .send(json!({"type": "usage.session_context", "request_id": "c2", "session_ids": fifty}))
        .await;
    assert_eq!(reply["type"], "usage.session_context.result", "50 個合法:{reply}");
}

#[tokio::test]
async fn session_context_without_a_folder_is_empty() {
    let harness = Harness::start_bare(None).await;

    let some = harness
        .send(json!({"type": "usage.session_context", "request_id": "c2", "session_ids": ["abc"]}))
        .await;

    assert_eq!(some, json!({"type": "usage.session_context.result", "request_id": "c2", "sessions": []}));
}

#[tokio::test]
async fn session_context_clamps_the_percentage_and_ignores_odd_sizes() {
    let harness = Harness::start().await;
    write_session(&harness, "a", now(), json!({"used_percentage": 250, "context_window_size": -5}));
    write_session(&harness, "b", now(), json!({"used_percentage": "40", "context_window_size": "big"}));

    let reply = harness
        .send(json!({"type": "usage.session_context", "request_id": "c1", "session_ids": ["a", "b"]}))
        .await;

    assert_eq!(reply["sessions"][0]["used_percentage"], 100.0, "{reply}");
    assert!(reply["sessions"][0].get("context_window_size").is_none(), "{reply}");
    assert!(reply["sessions"][1].get("used_percentage").is_none(), "{reply}");
    assert!(reply["sessions"][1].get("context_window_size").is_none(), "{reply}");
}

#[tokio::test]
async fn session_context_rejects_invalid_session_ids_without_reading_anything() {
    let harness = Harness::start().await;
    write_session(&harness, "good", now(), json!({"used_percentage": 1}));
    harness.write("secret.json", &session_file("secret", now(), json!({"used_percentage": 99})));
    let too_long = "a".repeat(129);
    for bad in [
        "",
        ".",
        "..",
        "../secret",
        "a/b",
        "a\\b",
        "with space",
        "貓",
        "a\u{0}b",
        too_long.as_str(),
        "a.json/../../secret",
    ] {
        let reply = harness
            .send(json!({"type": "usage.session_context", "request_id": "c1", "session_ids": ["good", bad]}))
            .await;

        assert_eq!(reply["type"], "error", "{bad:?}: {reply}");
        assert_eq!(reply["code"], "invalid_message", "{bad:?}: {reply}");
        assert_eq!(reply["request_id"], "c1");
    }
    let longest_valid = "a".repeat(128);
    write_session(&harness, &longest_valid, now(), json!({"used_percentage": 2}));
    let reply = harness
        .send(json!({"type": "usage.session_context", "request_id": "c2", "session_ids": [longest_valid, "..a", "a.."]}))
        .await;
    assert_eq!(
        reply["type"], "usage.session_context.result",
        "128 字以內的 A-Za-z0-9._- 都合法(只有 . 與 .. 不行):{reply}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn session_context_never_follows_symlinks_out_of_the_folder() {
    let harness = Harness::start().await;
    harness.write("outside.json", &session_file("link", now(), json!({"used_percentage": 99})));
    std::fs::create_dir_all(harness.path(".purrdr/claude-usage/sessions")).unwrap();
    std::os::unix::fs::symlink(harness.path("outside.json"), harness.path(".purrdr/claude-usage/sessions/link.json")).unwrap();

    let named = harness
        .send(json!({"type": "usage.session_context", "request_id": "c1", "session_ids": ["link"]}))
        .await;

    assert_eq!(named["sessions"], json!([]), "{named}");
}
