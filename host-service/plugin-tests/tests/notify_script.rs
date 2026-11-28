//! 主機外掛腳本(`host-plugin/`,raychiutw/purrdr.ios ADR-0004):herdr 在 `pane.agent_status_changed` 時以新程序執行它。
//!
//! 在暫存家目錄真的執行 `host-plugin/notify.sh`,herdr 給的環境變數照 herdr 文件
//! (`HERDR_PLUGIN_EVENT`、`HERDR_PLUGIN_EVENT_JSON`、`HERDR_PLUGIN_CONTEXT_JSON`、`HERDR_BIN_PATH`)手動帶入;
//! `curl` 與 `herdr` 換成家目錄 `bin/` 裡的假程式，記錄收到的參數與 stdin,不連網路、不碰真的 herdr。
//!
//! 移植自 app repo 的 `tests/PurrdrCoreTests/HostPluginScriptTests.swift`,每個測試的說明是原本的測試名稱。
//! `notify.sh` 只在 macOS／Linux 執行(manifest 限定平台),所以執行腳本的測試只在 Unix 跑;
//! Windows 的 `notify.ps1` 只測後台回應處理(`notify_ps1`,本機假 HTTP 後台)。manifest 的檢查三個平台都跑。

use std::path::{Path, PathBuf};

const PLUGIN_ID: &str = "purrdr.push";

/// 交叉編譯到別台執行時以 `PURRDR_HOST_PLUGIN_DIR` 指定。
fn plugin_directory() -> PathBuf {
    std::env::var_os("PURRDR_HOST_PLUGIN_DIR").map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../host-plugin"), PathBuf::from)
}

/// manifest:只掛 pane.agent_status_changed,macOS／Linux 用 sh、Windows 用 PowerShell 腳本(檔案都在)。
/// 解析 TOML 逐一比對每個事件：同一個事件名出現兩次，用字串 `contains` 會放過只改壞其中一個的情況。
#[test]
fn manifest_declares_hooks() {
    let text = std::fs::read_to_string(plugin_directory().join("herdr-plugin.toml")).unwrap();
    let manifest: toml::Table = text.parse().unwrap();
    assert_eq!(manifest["id"].as_str(), Some(PLUGIN_ID));
    // 0.2.0 起會讓位給主機服務的 lease(raychiutw/purrdr.host#48);主機服務靠這個版本號判斷舊 plugin 不能共存。
    // 0.2.1 起推播多帶 tab(raychiutw/purrdr.host#74);0.3.0 起帶工作秒數與方案用量(#96 #98)。
    assert_eq!(manifest["version"].as_str(), Some("0.3.0"));

    let events = manifest["events"].as_array().expect("沒有 [[events]]");
    let hooks: Vec<(&str, Vec<&str>, Vec<&str>)> = events
        .iter()
        .map(|event| {
            let strings = |key: &str| -> Vec<&str> {
                event[key]
                    .as_array()
                    .unwrap_or_else(|| panic!("事件缺 {key}"))
                    .iter()
                    .map(|value| value.as_str().unwrap())
                    .collect()
            };
            (event["on"].as_str().expect("事件缺 on"), strings("command"), strings("platforms"))
        })
        .collect();
    assert_eq!(
        hooks,
        vec![
            ("pane.agent_status_changed", vec!["sh", "notify.sh"], vec!["linux", "macos"]),
            (
                "pane.agent_status_changed",
                vec![
                    "powershell",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    "notify.ps1"
                ],
                vec!["windows"],
            ),
        ]
    );
    for script in ["notify.sh", "notify.ps1"] {
        assert!(plugin_directory().join(script).is_file(), "缺 {script}");
    }
}

/// notify.ps1 帶 UTF-8 BOM:Windows PowerShell 5.1 讀沒有 BOM 的檔案用系統字碼頁(如 cp950),中文註解會讓腳本解析失敗。
#[test]
fn powershell_script_has_utf8_bom() {
    let bytes = std::fs::read(plugin_directory().join("notify.ps1")).unwrap();
    assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "notify.ps1 缺 UTF-8 BOM");
}

#[cfg(unix)]
mod notify_sh {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    use serde_json::{Map, Value};
    use tempfile::TempDir;

    use super::plugin_directory;

    const ENDPOINT: &str = "https://backend.test/v1/push";

    fn key_a() -> String {
        "ab".repeat(32)
    }

    fn key_b() -> String {
        "cd".repeat(32)
    }

    /// herdr 序列化的事件 envelope(`EventEnvelope`:`event` 是 snake_case、`data` 以 `type` 標記)。
    /// `title` 是終端機標題，可能帶對話內容——外掛不能送出它。
    fn event(status: &str, pane: &str) -> String {
        format!(
            r#"{{"event":"pane_agent_status_changed","data":{{"type":"pane_agent_status_changed","pane_id":"{pane}","workspace_id":"w2","agent_status":"{status}","agent":"claude","title":"Delete \"prod\" database? secret-plan","display_agent":"Claude"}}}}"#
        )
    }

    fn blocked() -> String {
        event("blocked", "w2:p1")
    }

    /// herdr 事件沒帶 agent 名稱(欄位缺漏時)。
    fn blocked_without_agent() -> String {
        r#"{"event":"pane_agent_status_changed","data":{"type":"pane_agent_status_changed","pane_id":"w2:p1","workspace_id":"w2","agent_status":"blocked"}}"#
            .to_owned()
    }

    /// herdr `tab get` 的回覆(`label` 照 JSON 原樣放入)。
    fn tab_reply(label: &str) -> String {
        format!(
            r#"{{"id":"cli:tab:get","result":{{"tab":{{"agent_status":"blocked","focused":false,"label":"{label}","number":10,"pane_count":1,"tab_id":"w2:t3","workspace_id":"w2"}},"type":"tab_info"}}}}"#
        )
    }

    fn context(label: &str) -> String {
        format!(
            r#"{{"workspace_id":"w2","workspace_label":"{label}","focused_pane_id":"w2:p1","focused_pane_agent":"claude","focused_pane_status":"blocked","correlation_id":"evt-1"}}"#
        )
    }

    fn default_context() -> String {
        context("weather-app")
    }

    const FAKE_CURL: &str = r#"#!/bin/sh
n=$(ls "$HOME/curl-calls" 2>/dev/null | wc -l | tr -d ' ')
mkdir -p "$HOME/curl-calls/$n"
printf '%s\n' "$@" > "$HOME/curl-calls/$n/args"
cat > "$HOME/curl-calls/$n/stdin"
code=200
if [ -f "$HOME/backend-responses" ]; then
  while read -r key status; do
    grep -q "$key" "$HOME/curl-calls/$n/stdin" && code=$status
  done < "$HOME/backend-responses"
fi
# 送出期間這支裝置重新配對了(app 換了新密鑰)。
if [ -f "$HOME/repair-during-call/device" ]; then
  cp "$HOME/repair-during-call/pairing" "$HOME/.purrdr/push-$(cat "$HOME/repair-during-call/device")/pairing"
fi
printf '%s' "$code"
"#;

    /// 假 herdr:`pane get <pane>` 回傳該 pane 的 agent、session 與 tab id(錄自 herdr 0.9.5 的欄位形狀，
    /// 終端機標題 `terminal_title` 是對話內容，外掛不能送出);`tab get <tab>` 回傳 tab 的 label。
    /// 家目錄有 `herdr-pane-reply`／`herdr-tab-reply` 時改回那個檔案的內容;
    /// 沒有 `herdr-tab-reply` 時 `tab get` 失敗(模擬查不到 Tab label)。
    const FAKE_HERDR: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/herdr-calls"
if [ "$1 $2" = "pane get" ]; then
  if [ -f "$HOME/herdr-pane-reply" ]; then
    cat "$HOME/herdr-pane-reply"
  else
    printf '%s\n' '{"id":"cli:pane:get","result":{"pane":{"agent":"claude","agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"session-weather"},"agent_status":"blocked","pane_id":"w2:p1","tab_id":"w2:t3","terminal_title":"secret-plan","workspace_id":"w2"},"type":"pane_info"}}'
  fi
elif [ "$1 $2" = "tab get" ]; then
  [ -f "$HOME/herdr-tab-reply" ] || exit 1
  cat "$HOME/herdr-tab-reply"
fi
"#;

    struct CurlCall {
        arguments: Vec<String>,
        body: Map<String, Value>,
        raw_body: String,
    }

    /// 暫存家目錄，`bin/` 裡有假的 curl 與 herdr。
    struct Host {
        home: TempDir,
    }

    impl Host {
        fn new() -> Self {
            let host = Host {
                home: tempfile::tempdir().unwrap(),
            };
            host.install_fake("curl", FAKE_CURL);
            host.install_fake("herdr", FAKE_HERDR);
            host
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.home.path().join(relative)
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.path(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            path
        }

        fn install_fake(&self, name: &str, script: &str) {
            let path = self.write(&format!("bin/{name}"), script);
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        /// 與 app 的 `PushPairingFile.contents` 相同的格式。
        fn pair(&self, device: &str, key: &str, muted: &[&str]) {
            let mut muted: Vec<&str> = muted.to_vec();
            muted.sort();
            let mut lines = vec!["purrdr-push 1".to_owned(), format!("endpoint={ENDPOINT}"), format!("key={key}")];
            lines.extend(muted.iter().map(|session| format!("muted={session}")));
            self.write(&format!(".purrdr/push-{device}/pairing"), &(lines.join("\n") + "\n"));
        }

        /// 假後台對這把密鑰回應的 HTTP 狀態碼(沒設定的回 200)。
        fn respond(&self, key: &str, status: u16) {
            let path = self.path("backend-responses");
            let mut lines = std::fs::read_to_string(&path).unwrap_or_default();
            lines.push_str(&format!("{key} {status}\n"));
            std::fs::write(path, lines).unwrap();
        }

        fn is_paired(&self, device: &str) -> bool {
            self.path(&format!(".purrdr/push-{device}")).exists()
        }

        /// 以 herdr 的方式執行外掛：新程序、帶事件環境變數(沒有 `HERDR_SOCKET_PATH`)。
        fn run_hook(&self, event: &str, context: &str) -> i32 {
            self.run_hook_with_socket(event, context, None)
        }

        /// 同 `run_hook`,另外帶 herdr 注入的 `HERDR_SOCKET_PATH`。
        fn run_hook_with_socket(&self, event: &str, context: &str, socket: Option<&str>) -> i32 {
            let bin = self.path("bin");
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
            let status = Command::new("sh")
                .arg(plugin_directory().join("notify.sh"))
                .current_dir(plugin_directory())
                .env("HOME", self.home.path())
                .env("PATH", path)
                .env("HERDR_ENV", "1")
                .env("HERDR_PLUGIN_ID", "purrdr.push")
                .env("HERDR_PLUGIN_EVENT", "pane.agent_status_changed")
                .env("HERDR_PLUGIN_EVENT_JSON", event)
                .env("HERDR_PLUGIN_CONTEXT_JSON", context)
                .env("HERDR_BIN_PATH", bin.join("herdr"))
                .env_remove("HERDR_SOCKET_PATH")
                .envs(socket.map(|socket| ("HERDR_SOCKET_PATH", socket)))
                .status()
                .unwrap();
            status.code().unwrap_or(-1)
        }

        fn curl_calls(&self) -> Vec<CurlCall> {
            let directory = self.path("curl-calls");
            let Ok(entries) = std::fs::read_dir(&directory) else {
                return Vec::new();
            };
            let mut names: Vec<String> = entries.map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
            names.sort();
            names
                .iter()
                .map(|name| {
                    let raw_body = std::fs::read_to_string(directory.join(name).join("stdin")).unwrap();
                    let arguments = std::fs::read_to_string(directory.join(name).join("args"))
                        .unwrap()
                        .lines()
                        .filter(|line| !line.is_empty())
                        .map(str::to_owned)
                        .collect();
                    let body = match serde_json::from_str::<Value>(&raw_body) {
                        Ok(Value::Object(body)) => body,
                        _ => Map::new(),
                    };
                    CurlCall { arguments, body, raw_body }
                })
                .collect()
        }

        fn keys(&self) -> Vec<String> {
            self.curl_calls()
                .iter()
                .filter_map(|call| call.body.get("key")?.as_str().map(str::to_owned))
                .collect()
        }
    }

    /// Agent 進入「需要你」:帶配對密鑰呼叫後台，內容只有 workspace 名稱、狀態與 pane
    #[test]
    fn blocked_notifies_backend() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        assert_eq!(host.run_hook(&blocked(), &default_context()), 0);

        let calls = host.curl_calls();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.arguments.last().map(String::as_str), Some(ENDPOINT));
        let mut keys: Vec<&str> = call.body.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["key", "pane", "status", "tab", "workspace"]);
        assert_eq!(call.body["key"], key_a().as_str());
        assert_eq!(call.body["workspace"], "weather-app");
        assert_eq!(call.body["status"], "blocked");
        assert_eq!(call.body["pane"], "w2:p1");
        // 查不到 Tab label(假 herdr 的 tab get 失敗)→ 用 Agent 名稱。
        assert_eq!(call.body["tab"], "claude");
    }

    /// 推播請求不含終端機標題等對話內容
    #[test]
    fn request_carries_no_conversation_content() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.run_hook(&blocked(), &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert!(!call.raw_body.contains("secret-plan"));
        assert!(!call.raw_body.contains("prod"));
        assert!(!call.raw_body.contains("Claude"));
    }

    /// 配對密鑰經 stdin 交給 curl,不出現在指令參數(其他使用者用 ps 看得到參數)
    #[test]
    fn key_is_not_in_arguments() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.run_hook(&blocked(), &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert!(!call.arguments.join(" ").contains(&key_a()));
        assert!(call.arguments.iter().any(|argument| argument == "--max-time"));
    }

    /// Agent 完成：同樣帶配對密鑰呼叫後台(是否通知由後台依裝置偏好決定),狀態是 done
    #[test]
    fn done_notifies_backend() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        assert_eq!(host.run_hook(&event("done", "w2:p1"), &default_context()), 0);

        let calls = host.curl_calls();
        assert_eq!(calls.len(), 1);
        let mut keys: Vec<&str> = calls[0].body.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["key", "pane", "status", "tab", "workspace"]);
        assert_eq!(calls[0].body["key"], key_a().as_str());
        assert_eq!(calls[0].body["workspace"], "weather-app");
        assert_eq!(calls[0].body["status"], "done");
        assert_eq!(calls[0].body["pane"], "w2:p1");
        assert_eq!(calls[0].body["tab"], "claude");
        assert!(!calls[0].raw_body.contains("secret-plan"));
    }

    /// 完成時，靜音了這個聊天的裝置一樣跳過
    #[test]
    fn done_skips_muted_device() {
        let host = Host::new();
        host.pair("a1", &key_a(), &["session-weather"]);
        host.pair("b2", &key_b(), &[]);

        host.run_hook(&event("done", "w2:p1"), &default_context());

        assert_eq!(host.keys(), [key_b()]);
    }

    /// 完成時後台回 410 一樣刪掉那支裝置的配對
    #[test]
    fn done_revoked_pairing_is_removed() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.pair("b2", &key_b(), &[]);
        host.respond(&key_a(), 410);

        host.run_hook(&event("done", "w2:p1"), &default_context());

        assert!(!host.is_paired("a1"));
        assert!(host.is_paired("b2"));
    }

    /// 其他狀態(工作中、閒置)不通知
    #[test]
    fn other_statuses_do_nothing() {
        for status in ["working", "idle", "unknown", "", "blocked_x", "done2"] {
            let host = Host::new();
            host.pair("a1", &key_a(), &[]);

            assert_eq!(host.run_hook(&event(status, "w2:p1"), &default_context()), 0, "{status}");

            assert!(host.curl_calls().is_empty(), "{status} 不該通知");
        }
    }

    /// 沒有推播配對時什麼都不做
    #[test]
    fn no_pairing_does_nothing() {
        let host = Host::new();

        assert_eq!(host.run_hook(&blocked(), &default_context()), 0);
        assert!(host.curl_calls().is_empty());
    }

    /// 一台主機配對多支裝置：每支各送一次，用各自的密鑰
    #[test]
    fn every_paired_device_is_notified() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.pair("b2", &key_b(), &[]);

        host.run_hook(&blocked(), &default_context());

        let mut keys = host.keys();
        keys.sort();
        assert_eq!(keys, [key_a(), key_b()]);
    }

    /// 某支裝置靜音了這個聊天(session):只跳過那支裝置
    #[test]
    fn muted_chat_skips_that_device() {
        let host = Host::new();
        host.pair("a1", &key_a(), &["session-weather"]);
        host.pair("b2", &key_b(), &["session-other"]);

        host.run_hook(&blocked(), &default_context());

        assert_eq!(host.keys(), [key_b()]);
        let herdr_calls = std::fs::read_to_string(host.path("herdr-calls")).unwrap();
        assert!(herdr_calls.contains("pane get w2:p1"));
    }

    /// 沒有配對裝置時不必問 herdr(沒人要收推播，不為它查 Tab)
    #[test]
    fn without_pairings_herdr_is_not_asked() {
        let host = Host::new();

        host.run_hook(&blocked(), &default_context());

        assert!(!host.path("herdr-calls").exists());
    }

    /// 多支裝置也只問 herdr 一次(pane get 與 tab get 各一次)
    #[test]
    fn herdr_is_asked_once_for_all_devices() {
        let host = Host::new();
        host.pair("a1", &key_a(), &["session-other"]);
        host.pair("b2", &key_b(), &[]);

        host.run_hook(&blocked(), &default_context());

        let herdr_calls = std::fs::read_to_string(host.path("herdr-calls")).unwrap();
        assert_eq!(herdr_calls.lines().collect::<Vec<_>>(), ["pane get w2:p1", "tab get w2:t3"]);
    }

    /// 有 Tab label:`tab` 用它(不帶 Agent 名稱、終端機標題)
    #[test]
    fn tab_label_is_sent_when_the_tab_has_one() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write("herdr-tab-reply", &tab_reply("後台 API"));

        host.run_hook(&blocked(), &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert_eq!(call.body["tab"], "後台 API");
        assert!(!call.raw_body.contains("secret-plan"));
    }

    /// Tab label 含引號與反斜線時仍送出合法 JSON,名稱不變
    #[test]
    fn tab_label_escapes_survive() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write("herdr-tab-reply", &tab_reply(r#"fix \"login\" \\ bug"#));

        host.run_hook(&blocked(), &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert_eq!(call.body["tab"], r#"fix "login" \ bug"#);
    }

    /// 後台的 `tab` 最長 100 字元(超過回 400):以 UTF-16 單位計 100 截斷，不切壞 UTF-8、不切開代理對;剛好 100 不動
    #[test]
    fn long_tab_label_is_truncated_to_100_utf16_units() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        for (label, expected) in [
            ("天".repeat(150), "天".repeat(100)),
            ("天".repeat(100), "天".repeat(100)),
            ("a".repeat(101), "a".repeat(100)),
            // é 算 1 個單位、🙂 算 2 個:33 組是 99 個單位，再放 é 剛好 100。
            ("é🙂".repeat(80), format!("{}é", "é🙂".repeat(33))),
            // 🙂 跨過第 100 個單位:整個捨棄，不留半個代理對。
            (format!("{}🙂tail", "a".repeat(99)), "a".repeat(99)),
            (format!("{}🙂", "a".repeat(98)), format!("{}🙂", "a".repeat(98))),
            // 截斷後尾端剩空白要再去掉。
            (format!("{} 🙂tail", "a".repeat(98)), "a".repeat(98)),
        ] {
            host.write("herdr-tab-reply", &tab_reply(&label));
            host.run_hook(&blocked(), &default_context());
            let calls = host.curl_calls();
            let call = calls.last().expect("有送出推播");
            assert_eq!(call.body["tab"], expected.as_str(), "{label}");
        }
    }

    /// 截斷點在跳脫序列(`\"`、`\uXXXX`)上時，整個序列算一個字元、不被切開，送出的仍是合法 JSON
    #[test]
    fn truncation_never_splits_a_json_escape() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        for (label, expected) in [
            (format!("{}\\\"tail", "a".repeat(99)), format!("{}\"", "a".repeat(99))),
            (format!("{}\\u00e9tail", "a".repeat(99)), format!("{}é", "a".repeat(99))),
            (format!("{}\\\\tail", "a".repeat(99)), format!("{}\\", "a".repeat(99))),
            // 跳脫的代理對(`\ud83d\ude00` = 😀)算 2 個單位:跨過第 100 個時整對捨棄，不能只留高位。
            (format!("{}\\ud83d\\ude00tail", "a".repeat(99)), "a".repeat(99)),
            (format!("{}\\ud83d\\ude00tail", "a".repeat(98)), format!("{}😀", "a".repeat(98))),
            // 逸出的反斜線後面接 `ud83d` 字樣是一般文字，不是代理對。
            (format!("{}\\\\ud83dtail", "a".repeat(94)), format!("{}\\ud83d", "a".repeat(94))),
        ] {
            host.write("herdr-tab-reply", &tab_reply(&label));
            host.run_hook(&blocked(), &default_context());
            let calls = host.curl_calls();
            let call = calls.last().expect("有送出推播");
            assert_eq!(call.body.get("tab").and_then(Value::as_str), Some(expected.as_str()), "{}", call.raw_body);
        }
    }

    /// 只有空白的 Tab label 等於沒有:用 Agent 名稱;前後空白去掉
    #[test]
    fn blank_tab_label_falls_back_and_surrounding_spaces_are_trimmed() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.write("herdr-tab-reply", &tab_reply("   "));
        host.run_hook(&blocked(), &default_context());
        host.write("herdr-tab-reply", &tab_reply("  後台 API  "));
        host.run_hook(&blocked(), &default_context());

        let tabs: Vec<Value> = host.curl_calls().iter().map(|call| call.body["tab"].clone()).collect();
        assert_eq!(tabs, ["claude", "後台 API"].map(Value::from));
    }

    /// 只有空白的 Agent 名稱也省略 `tab`
    #[test]
    fn blank_agent_name_omits_the_tab() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let event = r#"{"event":"pane_agent_status_changed","data":{"type":"pane_agent_status_changed","pane_id":"w2:p1","workspace_id":"w2","agent_status":"blocked","agent":"  "}}"#;
        host.write(
            "herdr-pane-reply",
            r#"{"id":"cli:pane:get","result":{"pane":{"pane_id":"w2:p1","tab_id":"w2:t3","workspace_id":"w2"},"type":"pane_info"}}"#,
        );

        host.run_hook(event, &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert!(!call.body.contains_key("tab"), "{}", call.raw_body);
    }

    /// herdr 預設的 Tab 名稱是順序號(`1`、`2`…),不是使用者取的名字:當作沒有 label,改用 Agent 名稱
    #[test]
    fn numeric_default_tab_label_falls_back_to_agent_name() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write("herdr-tab-reply", &tab_reply("2"));

        host.run_hook(&blocked(), &default_context());

        assert_eq!(host.curl_calls().first().map(|call| call.body["tab"].clone()), Some(Value::from("claude")));
    }

    /// 沒有 Tab label(空字串)用該 Pane 的 Agent 名稱;事件沒帶 agent 時改用 pane get 的 agent
    #[test]
    fn missing_tab_label_falls_back_to_agent_name() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write("herdr-tab-reply", &tab_reply(""));

        host.run_hook(&blocked(), &default_context());
        host.run_hook(&blocked_without_agent(), &default_context());

        let tabs: Vec<Value> = host.curl_calls().iter().map(|call| call.body["tab"].clone()).collect();
        assert_eq!(tabs, [Value::from("claude"), Value::from("claude")]);
    }

    /// Tab label 與 Agent 名稱都沒有:省略 `tab`(其餘欄位照送)
    #[test]
    fn tab_is_omitted_when_there_is_no_label_and_no_agent() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write(
            "herdr-pane-reply",
            r#"{"id":"cli:pane:get","result":{"pane":{"pane_id":"w2:p1","tab_id":"w2:t3","workspace_id":"w2"},"type":"pane_info"}}"#,
        );

        host.run_hook(&blocked_without_agent(), &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert!(!call.body.contains_key("tab"), "{}", call.raw_body);
        assert_eq!(call.body["workspace"], "weather-app");
    }

    /// 沒有 pane id 時不問 herdr,`tab` 用事件裡的 Agent 名稱
    #[test]
    fn without_a_pane_the_agent_name_is_still_sent() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.run_hook(&event("blocked", "w2:p1;rm"), &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert_eq!(call.body["tab"], "claude");
        assert!(!host.path("herdr-calls").exists());
    }

    /// workspace 名稱含引號與反斜線時仍送出合法 JSON,名稱不變
    #[test]
    fn workspace_label_escapes_survive() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.run_hook(&blocked(), &context(r#"my \"app\" \\ 天氣"#));

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert_eq!(call.body["workspace"], r#"my "app" \ 天氣"#);
    }

    /// 沒有 workspace 名稱時用 workspace id
    #[test]
    fn missing_label_falls_back_to_workspace_id() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.run_hook(&blocked(), r#"{"workspace_id":"w2"}"#);

        assert_eq!(host.curl_calls().first().map(|call| call.body["workspace"].clone()), Some(Value::from("w2")));
    }

    /// pane id 形狀不對時不送 pane
    #[test]
    fn malformed_pane_is_omitted() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.run_hook(&event("blocked", "w2:p1;rm"), &default_context());

        let calls = host.curl_calls();
        let call = calls.first().expect("有送出推播");
        assert!(!call.body.contains_key("pane"));
    }

    /// 後台回 410(配對已撤銷):刪掉那支裝置的配對資料夾，其他裝置照送
    #[test]
    fn revoked_pairing_is_removed() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.pair("b2", &key_b(), &[]);
        host.respond(&key_a(), 410);

        assert_eq!(host.run_hook(&blocked(), &default_context()), 0);

        let mut keys = host.keys();
        keys.sort();
        assert_eq!(keys, [key_a(), key_b()]);
        assert!(!host.is_paired("a1"));
        assert!(host.is_paired("b2"));
    }

    /// 送出後才重新配對(新密鑰)時，舊密鑰的 410 不刪新的配對
    #[test]
    fn revoked_old_key_keeps_a_pairing_renewed_during_the_call() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.respond(&key_a(), 410);
        host.write("repair-during-call/device", "a1");
        host.write("repair-during-call/pairing", &format!("purrdr-push 1\nendpoint={ENDPOINT}\nkey={}\n", key_b()));

        host.run_hook(&blocked(), &default_context());

        assert!(host.is_paired("a1"));
    }

    /// 撤銷的配對刪掉後，下次不再對它呼叫後台
    #[test]
    fn removed_pairing_is_not_called_again() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.respond(&key_a(), 410);

        host.run_hook(&blocked(), &default_context());
        host.run_hook(&blocked(), &default_context());

        assert_eq!(host.keys(), [key_a()]);
    }

    /// 後台其他錯誤(暫時性)不刪配對
    #[test]
    fn other_backend_errors_keep_pairing() {
        for status in [400, 401, 404, 429, 500, 503] {
            let host = Host::new();
            host.pair("a1", &key_a(), &[]);
            host.respond(&key_a(), status);

            host.run_hook(&blocked(), &default_context());

            assert!(host.is_paired("a1"), "{status} 不該刪配對");
        }
    }

    // MARK: - 讓位給主機服務(raychiutw/purrdr.host#48)

    /// 假的 herdr socket 路徑(只比對字串，不需要真的有 socket)。
    fn socket_in(host: &Host) -> String {
        let directory = host.path("herdr-config");
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("herdr.sock").to_string_lossy().into_owned()
    }

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    impl Host {
        /// 主機服務的 lease(`~/.purrdr/host-service/push-sender`):記觀察的 socket 與更新時間(Unix 秒)。
        fn lease(&self, socket: &str, updated: u64) {
            self.write(
                ".purrdr/host-service/push-sender",
                &format!("purrdr-lease 1\nsocket={socket}\nupdated={updated}\n"),
            );
        }
    }

    /// 主機服務持有新鮮、同一個 socket 的 lease:外掛讓位，不呼叫後台。
    #[test]
    fn fresh_lease_for_the_same_socket_makes_the_plugin_yield() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);
        host.lease(&socket, now());

        assert_eq!(host.run_hook_with_socket(&blocked(), &default_context(), Some(&socket)), 0);

        assert!(host.curl_calls().is_empty(), "lease 新鮮、同 socket 時不該送");
    }

    /// 同一個 socket 經不同寫法(`..`、符號連結的資料夾)也算同一個。
    #[test]
    fn socket_paths_are_compared_after_canonicalizing() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);
        std::os::unix::fs::symlink(host.path("herdr-config"), host.path("config-link")).unwrap();
        host.lease(&socket, now());
        let spelled_differently = format!("{}/config-link/../herdr-config/herdr.sock", host.path("").display());

        host.run_hook_with_socket(&blocked(), &default_context(), Some(&spelled_differently));
        assert!(host.curl_calls().is_empty(), "..／符號連結的寫法應視為同一個 socket");

        let linked = format!("{}/config-link/herdr.sock", host.path("").display());
        host.run_hook_with_socket(&blocked(), &default_context(), Some(&linked));
        assert!(host.curl_calls().is_empty(), "符號連結的資料夾應視為同一個 socket");
    }

    /// lease 過期(超過 120 秒沒更新，主機服務可能掛了):外掛接回。
    #[test]
    fn stale_lease_does_not_stop_the_plugin() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);
        host.lease(&socket, now() - 121);

        host.run_hook_with_socket(&blocked(), &default_context(), Some(&socket));

        assert_eq!(host.keys(), [key_a()]);
    }

    /// 剛好在門檻內(119 秒前)還算新鮮。
    #[test]
    fn lease_just_inside_the_window_still_counts() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);
        host.lease(&socket, now() - 100);

        host.run_hook_with_socket(&blocked(), &default_context(), Some(&socket));

        assert!(host.curl_calls().is_empty());
    }

    /// lease 觀察的是別的 herdr socket(另一個 session):這個事件主機服務不會送，外掛照送。
    #[test]
    fn lease_for_another_socket_does_not_stop_the_plugin() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);
        host.lease(&format!("{}-other", socket), now());

        host.run_hook_with_socket(&blocked(), &default_context(), Some(&socket));

        assert_eq!(host.keys(), [key_a()]);
    }

    /// herdr 沒給 `HERDR_SOCKET_PATH`:無法確認是同一個 socket，寧可重複也不漏推。
    #[test]
    fn missing_socket_environment_never_yields() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.lease(&socket_in(&host), now());

        host.run_hook(&blocked(), &default_context());

        assert_eq!(host.keys(), [key_a()]);
    }

    /// 壞掉的 lease(沒有 socket、時間不是數字、時間在未來)一律當作沒有。
    #[test]
    fn malformed_leases_are_ignored() {
        let socket_text = |host: &Host| socket_in(host);
        let cases: [fn(&str) -> String; 5] = [
            |_| String::new(),
            |socket| format!("socket={socket}\n"),
            |socket| format!("socket={socket}\nupdated=yesterday\n"),
            |_| format!("updated={}\n", now()),
            |socket| format!("socket={socket}\nupdated={}\n", now() + 3600),
        ];
        for (index, contents) in cases.iter().enumerate() {
            let host = Host::new();
            host.pair("a1", &key_a(), &[]);
            let socket = socket_text(&host);
            host.write(".purrdr/host-service/push-sender", &contents(&socket));

            host.run_hook_with_socket(&blocked(), &default_context(), Some(&socket));

            assert_eq!(host.keys(), [key_a()], "第 {index} 種壞 lease 不該讓位");
        }
    }

    /// 沒有 lease(沒有主機服務，例如 SSH 主機):行為與 0.1.0 相同。
    #[test]
    fn without_a_lease_the_plugin_behaves_as_before() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);

        host.run_hook_with_socket(&blocked(), &default_context(), Some(&socket));

        assert_eq!(host.keys(), [key_a()]);
    }

    // MARK: - 完成帶工作秒數(raychiutw/purrdr.host#96)與方案用量(#98)

    impl Host {
        /// 外掛自己的狀態檔(不以 `push-` 開頭，不會被當成配對資料夾)。
        fn work_file(&self) -> PathBuf {
            self.path(".purrdr/plugin-state/work-w2_p1")
        }

        /// 預先放一筆「開始工作」時間，模擬外掛在 `seconds_ago` 秒前看到 working。
        fn started(&self, seconds_ago: u64) {
            self.write(".purrdr/plugin-state/work-w2_p1", &format!("working_since={}\n", now() - seconds_ago));
        }

        fn run_status(&self, status: &str) -> i32 {
            self.run_hook(&event(status, "w2:p1"), &default_context())
        }

        /// 假 Claude status line 用量檔(`~/.purrdr/claude-usage/usage.json`，格式見 `claude-usage/statusline.sh`)。
        fn usage(&self, five_hour: &str, seven_day: &str) {
            self.write(
                ".purrdr/claude-usage/usage.json",
                &format!(
                    "{{\"captured_at\":{},\"status_line_input\":{{\"session_id\":\"s\",\"rate_limits\":{{\"five_hour\":{five_hour},\"seven_day\":{seven_day}}}}}}}\n",
                    now()
                ),
            );
        }

        fn usage_calls(&self) -> Vec<Map<String, Value>> {
            self.curl_calls()
                .into_iter()
                .map(|call| call.body)
                .filter(|body| body["status"] == "usage")
                .collect()
        }
    }

    fn period(percentage: &str, resets_at: u64) -> String {
        format!("{{\"used_percentage\":{percentage},\"resets_at\":{resets_at}}}")
    }

    fn idle_period() -> String {
        period("10", now() + 3 * 3600)
    }

    fn work_seconds_of(host: &Host) -> Option<Value> {
        host.curl_calls().first().expect("有送出推播").body.get("workSeconds").cloned()
    }

    /// working 事件:只記開始時間(在外掛狀態檔，不是 push-*),不對後台送任何東西
    #[test]
    fn working_records_the_start_and_sends_nothing() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        assert_eq!(host.run_status("working"), 0);

        assert!(host.curl_calls().is_empty());
        let recorded = std::fs::read_to_string(host.work_file()).expect("有記開始時間");
        assert!(recorded.starts_with("working_since="), "{recorded}");
        let pairings: Vec<String> = std::fs::read_dir(host.path(".purrdr"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("push-"))
            .collect();
        assert_eq!(pairings, ["push-a1"]);
    }

    /// working 之後 done:done 帶 workSeconds(整數、非負),並清掉開始時間
    #[test]
    fn done_after_working_carries_work_seconds_and_clears_the_start() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);

        host.run_status("working");
        host.run_status("done");

        let seconds = work_seconds_of(&host).expect("done 帶 workSeconds");
        assert!(seconds.as_u64().is_some_and(|seconds| seconds <= 10), "{seconds}");
        assert!(!host.work_file().exists());
    }

    /// 開始時間在 120 秒前:workSeconds 約 120
    #[test]
    fn done_reports_elapsed_seconds_from_the_recorded_start() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.started(120);

        host.run_status("done");

        let seconds = work_seconds_of(&host).and_then(|seconds| seconds.as_u64()).expect("done 帶 workSeconds");
        assert!((120..=130).contains(&seconds), "{seconds}");
    }

    /// 算不出來一律省略欄位(不帶 0):done 先於 working、重啟後沒有開始時間、已經用掉、重複 working、時間倒退、檔案壞掉、太久以前
    #[test]
    fn work_seconds_is_omitted_when_it_cannot_be_computed() {
        // 沒有開始時間(重啟後遺失，或 done 先於 working)。
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.run_status("done");
        assert_eq!(work_seconds_of(&host), None);

        // 同一段工作只算一次:第二個 done 沒有開始時間。
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.started(60);
        host.run_status("done");
        host.run_status("done");
        assert_eq!(host.curl_calls().last().unwrap().body.get("workSeconds"), None);

        // 重複 working(中間沒有 done):順序不可信，這一段省略。
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.run_status("working");
        host.run_status("working");
        host.run_status("done");
        assert_eq!(work_seconds_of(&host), None);

        // 時間倒退(開始時間比現在晚)。
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write(".purrdr/plugin-state/work-w2_p1", &format!("working_since={}\n", now() + 3600));
        host.run_status("done");
        assert_eq!(work_seconds_of(&host), None);

        // 壞掉的狀態檔、前導零、太久以前的殘留。
        for contents in [
            "",
            "working_since=abc\n",
            "working_since=08\n",
            "working_since=-5\n",
            &format!("working_since={}\n", now() - 3 * 86400),
        ] {
            let host = Host::new();
            host.pair("a1", &key_a(), &[]);
            host.write(".purrdr/plugin-state/work-w2_p1", contents);
            host.run_status("done");
            assert_eq!(work_seconds_of(&host), None, "{contents:?}");
        }
    }

    /// 太久以前的殘留開始時間(例如 pane 當掉沒收到 done)遇到新的 working 時直接換成新的，不誤判成重複
    #[test]
    fn a_stale_start_is_replaced_by_a_new_working_event() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write(".purrdr/plugin-state/work-w2_p1", &format!("working_since={}\n", now() - 3 * 86400));

        host.run_status("working");
        host.run_status("done");

        let seconds = work_seconds_of(&host).and_then(|seconds| seconds.as_u64()).expect("done 帶 workSeconds");
        assert!(seconds <= 10, "{seconds}");
    }

    /// 重複 working 寫下的 invalid 標記也要在 24 小時後過期(以檔案修改時間判斷),否則該 pane 之後每次 working 都重寫 invalid
    #[test]
    fn an_invalid_marker_expires_after_a_day() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write(".purrdr/plugin-state/work-w2_p1", "working_since=invalid\n");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86400);
        std::fs::File::options().write(true).open(host.work_file()).unwrap().set_modified(old).unwrap();

        host.run_status("working");
        host.run_status("done");

        let seconds = work_seconds_of(&host).and_then(|seconds| seconds.as_u64()).expect("done 帶 workSeconds");
        assert!(seconds <= 10, "{seconds}");
    }

    /// invalid 標記還沒過期時,重複的 working 不重寫它(否則修改時間被刷新,永遠不會過期)
    #[test]
    fn a_fresh_invalid_marker_keeps_its_modified_time_on_repeated_working() {
        let host = Host::new();
        host.write(".purrdr/plugin-state/work-w2_p1", "working_since=invalid\n");
        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(host.work_file())
            .unwrap()
            .set_modified(earlier)
            .unwrap();

        host.run_status("working");

        let modified = std::fs::metadata(host.work_file()).unwrap().modified().unwrap();
        assert_eq!(modified, earlier);
    }

    /// blocked 結束這一段工作(使用者介入):之後再 working 是新的一段，done 只算新的一段
    #[test]
    fn blocked_ends_the_work_segment() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.started(3000);
        host.run_status("blocked");
        assert!(!host.work_file().exists());

        host.run_status("working");
        host.run_status("done");

        let seconds = host.curl_calls().last().unwrap().body["workSeconds"].as_u64().expect("done 帶 workSeconds");
        assert!(seconds <= 10, "{seconds}");
    }

    /// 不同 pane 各自記;pane id 形狀不對時不記也不帶
    #[test]
    fn each_pane_has_its_own_start_and_malformed_pane_has_none() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.started(200);

        host.run_hook(&event("done", "w2:p9"), &default_context());
        host.run_hook(&event("done", "w2:p1;rm"), &default_context());

        assert!(host.curl_calls().iter().all(|call| !call.body.contains_key("workSeconds")));
        assert!(host.work_file().exists(), "別的 pane 的 done 不動 w2:p1 的紀錄");
    }

    /// 主機服務讓位(lease 新鮮):不送 done(也就不帶 workSeconds),但仍清掉開始時間;working 照記(lease 失效後才接得上)
    #[test]
    fn fresh_lease_yields_work_seconds_but_still_tracks_the_start() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);
        host.lease(&socket, now());

        host.run_hook_with_socket(&event("working", "w2:p1"), &default_context(), Some(&socket));
        assert!(host.work_file().exists());
        host.run_hook_with_socket(&event("done", "w2:p1"), &default_context(), Some(&socket));

        assert!(host.curl_calls().is_empty());
        assert!(!host.work_file().exists());
    }

    /// 用量 85%:送一則 usage，內容只有 key/status/agent/threshold/resetsAt,不帶 workspace、pane、tab
    #[test]
    fn usage_crossing_80_is_sent_without_workspace_pane_or_tab() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let resets = now() + 3 * 3600;
        host.usage(&period("85.5", resets), &idle_period());

        host.run_status("working");

        let calls = host.curl_calls();
        assert_eq!(calls.len(), 1, "只送一則,working 本身不推播");
        let mut keys: Vec<&str> = calls[0].body.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["agent", "key", "resetsAt", "status", "threshold"]);
        assert_eq!(calls[0].body["key"], key_a().as_str());
        assert_eq!(calls[0].body["status"], "usage");
        assert_eq!(calls[0].body["agent"], "claude");
        assert_eq!(calls[0].body["threshold"], 80);
        assert_eq!(calls[0].body["resetsAt"], resets);
    }

    /// 同一週期同一門檻只送一次(之後每次狀態改變都不重送);跨過 95 再補送 95
    #[test]
    fn each_threshold_is_sent_once_per_cycle() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let resets = now() + 3 * 3600;
        host.usage(&period("82", resets), &idle_period());

        host.run_status("working");
        host.run_status("idle");
        host.run_status("working");
        assert_eq!(host.usage_calls().len(), 1);

        host.usage(&period("96", resets), &idle_period());
        host.run_status("working");
        host.run_status("working");

        let thresholds: Vec<Value> = host.usage_calls().iter().map(|body| body["threshold"].clone()).collect();
        assert_eq!(thresholds, [80, 95].map(Value::from));
    }

    /// 一次跨過兩個門檻:80 與 95 各送一則;新的週期(resetsAt 不同)重新計
    #[test]
    fn crossing_both_thresholds_sends_two_and_a_new_cycle_resets_the_record() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.usage(&period("97", now() + 3600), &idle_period());

        host.run_status("working");
        assert_eq!(
            host.usage_calls().iter().map(|body| body["threshold"].clone()).collect::<Vec<_>>(),
            [80, 95].map(Value::from)
        );

        let next = now() + 5 * 3600;
        host.usage(&period("90", next), &idle_period());
        host.run_status("working");

        let calls = host.usage_calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            (calls[2]["threshold"].clone(), calls[2]["resetsAt"].clone()),
            (Value::from(80), Value::from(next))
        );
    }

    /// 五小時與七天兩個視窗各自判斷
    #[test]
    fn both_periods_are_checked() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let week = now() + 5 * 86400;
        host.usage(&idle_period(), &period("96", week));

        host.run_status("idle");

        let calls = host.usage_calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|body| body["resetsAt"] == week));
    }

    /// 沒有用量檔、低於門檻、沒有重置時間、已過重置時間、檔案壞掉:不送
    #[test]
    fn usage_is_not_sent_without_a_usable_reading() {
        let future = now() + 3600;
        let cases = [
            None,
            Some((idle_period(), idle_period())),
            Some((period("99", now() - 10), period("99", now() - 10))),
            Some(("{\"used_percentage\":99}".to_owned(), "{\"used_percentage\":99}".to_owned())),
            Some((format!("{{\"used_percentage\":99,\"resets_at\":\"{future}\"}}"), idle_period())),
        ];
        for case in cases {
            let host = Host::new();
            host.pair("a1", &key_a(), &[]);
            if let Some((five, seven)) = &case {
                host.usage(five, seven);
            }
            host.run_status("working");
            assert!(host.curl_calls().is_empty(), "{case:?}");
        }
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.write(".purrdr/claude-usage/usage.json", "not json {");
        host.run_status("working");
        assert!(host.curl_calls().is_empty());
    }

    /// 主機服務讓位(lease 新鮮)時 usage 也不送;lease 失效後才由外掛送
    #[test]
    fn usage_yields_to_a_fresh_host_service_lease() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        let socket = socket_in(&host);
        host.usage(&period("90", now() + 3600), &idle_period());
        host.lease(&socket, now());

        host.run_hook_with_socket(&event("working", "w2:p1"), &default_context(), Some(&socket));
        assert!(host.curl_calls().is_empty());

        host.lease(&socket, now() - 600);
        host.run_hook_with_socket(&event("working", "w2:p1"), &default_context(), Some(&socket));
        assert_eq!(host.usage_calls().len(), 1);
    }

    /// 沒有配對裝置:不送，也不記「已送過」(之後配對了還是會送)
    #[test]
    fn usage_is_not_marked_sent_when_nobody_received_it() {
        let host = Host::new();
        host.usage(&period("90", now() + 3600), &idle_period());
        host.run_status("working");
        assert!(host.curl_calls().is_empty());

        host.pair("a1", &key_a(), &[]);
        host.run_status("working");

        assert_eq!(host.usage_calls().len(), 1);
    }

    /// usage 後台回 410:刪掉配對，同一事件的 blocked 推播也不再對它送
    #[test]
    fn usage_revoked_pairing_is_removed() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.respond(&key_a(), 410);
        host.usage(&period("90", now() + 3600), &idle_period());

        host.run_status("blocked");

        assert!(!host.is_paired("a1"));
        assert_eq!(host.curl_calls().len(), 1);
    }

    /// blocked 與用量同時發生:usage 與 blocked 各一則，blocked 內容不變
    #[test]
    fn usage_and_blocked_are_sent_separately() {
        let host = Host::new();
        host.pair("a1", &key_a(), &[]);
        host.usage(&period("90", now() + 3600), &idle_period());

        host.run_status("blocked");

        let calls = host.curl_calls();
        assert_eq!(calls.len(), 2);
        let blocked: Vec<_> = calls.iter().filter(|call| call.body["status"] == "blocked").collect();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].body["workspace"], "weather-app");
    }
}

/// `notify.ps1` 的後台回應處理：真的以 Windows PowerShell 執行，後台換成本機的假 HTTP 伺服器。
#[cfg(windows)]
mod notify_ps1 {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::{Arc, Mutex};

    use serde_json::Value;
    use tempfile::TempDir;

    use super::plugin_directory;

    /// `agent` 是 herdr 事件帶的 Agent 名稱(None 是欄位缺漏)。
    fn event_with(status: &str, agent: Option<&str>) -> String {
        let agent = agent.map(|agent| format!(r#","agent":"{agent}""#)).unwrap_or_default();
        format!(
            r#"{{"event":"pane_agent_status_changed","data":{{"type":"pane_agent_status_changed","pane_id":"w2:p1","workspace_id":"w2","agent_status":"{status}"{agent}}}}}"#
        )
    }

    /// 假 herdr(`herdr.cmd`,放在暫存家目錄):`pane get` 與 `tab get` 回傳家目錄裡 `herdr-pane-reply`／`herdr-tab-reply` 的內容,
    /// 檔案不存在就失敗(模擬 herdr 查不到)。
    const FAKE_HERDR: &str = "@echo off\r\nif \"%1 %2\"==\"pane get\" (\r\n  if exist \"%USERPROFILE%\\herdr-pane-reply\" (type \"%USERPROFILE%\\herdr-pane-reply\") else (exit /b 1)\r\n) else if \"%1 %2\"==\"tab get\" (\r\n  if exist \"%USERPROFILE%\\herdr-tab-reply\" (type \"%USERPROFILE%\\herdr-tab-reply\") else (exit /b 1)\r\n)\r\n";
    const PANE_REPLY: &str = r#"{"id":"cli:pane:get","result":{"pane":{"agent":"claude","agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"session-weather"},"pane_id":"w2:p1","tab_id":"w2:t3","terminal_title":"secret-plan","workspace_id":"w2"},"type":"pane_info"}}"#;

    fn tab_reply(label: &str) -> String {
        format!(r#"{{"id":"cli:tab:get","result":{{"tab":{{"label":"{label}","number":10,"tab_id":"w2:t3","workspace_id":"w2"}},"type":"tab_info"}}}}"#)
    }
    const CONTEXT: &str = r#"{"workspace_id":"w2","workspace_label":"weather-app"}"#;

    /// 依請求本文的 `key` 回應指定狀態碼(沒設定的回 200),記下收到的本文。
    struct Backend {
        endpoint: String,
        received: Arc<Mutex<Vec<Value>>>,
    }

    impl Backend {
        fn start(responses: HashMap<String, u16>) -> Self {
            Backend::start_renewing(responses, None)
        }

        /// 回應前先把 `renewal` 的內容寫進那個檔案(送出期間重新配對)。
        fn start_renewing(responses: HashMap<String, u16>, renewal: Option<(PathBuf, String)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/v1/push", listener.local_addr().unwrap());
            let received = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&received);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':')
                            && name.eq_ignore_ascii_case("content-length")
                        {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0; length];
                    if reader.read_exact(&mut body).is_err() {
                        continue;
                    }
                    let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
                    let key = body["key"].as_str().unwrap_or_default().to_owned();
                    let status = responses.get(&key).copied().unwrap_or(200);
                    recorder.lock().unwrap().push(body);
                    if let Some((path, contents)) = &renewal {
                        std::fs::write(path, contents).unwrap();
                    }
                    let reply = format!("HTTP/1.1 {status} Status\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}");
                    let _ = stream.write_all(reply.as_bytes());
                }
            });
            Backend { endpoint, received }
        }

        fn keys(&self) -> Vec<String> {
            let mut keys: Vec<String> = self
                .received
                .lock()
                .unwrap()
                .iter()
                .map(|body| body["key"].as_str().unwrap_or_default().to_owned())
                .collect();
            keys.sort();
            keys
        }

        fn bodies(&self) -> Vec<Value> {
            self.received.lock().unwrap().clone()
        }

        fn statuses(&self) -> Vec<String> {
            self.received
                .lock()
                .unwrap()
                .iter()
                .map(|body| body["status"].as_str().unwrap_or_default().to_owned())
                .collect()
        }
    }

    struct Host {
        home: TempDir,
    }

    impl Host {
        fn new() -> Self {
            Host {
                home: tempfile::tempdir().unwrap(),
            }
        }

        fn pairing_directory(&self, device: &str) -> PathBuf {
            self.home.path().join(".purrdr").join(format!("push-{device}"))
        }

        fn pair(&self, device: &str, key: &str, endpoint: &str) {
            let directory = self.pairing_directory(device);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("pairing"), format!("purrdr-push 1\nendpoint={endpoint}\nkey={key}\n")).unwrap();
        }

        fn run_hook(&self) -> i32 {
            self.run_hook_with_status("blocked")
        }

        fn run_hook_with_status(&self, status: &str) -> i32 {
            self.run_hook_with(status, None)
        }

        /// 同 `run_hook_with_status`,另外帶 herdr 注入的 `HERDR_SOCKET_PATH`。
        fn run_hook_with(&self, status: &str, socket: Option<&str>) -> i32 {
            self.run_hook_full(status, None, socket)
        }

        /// 同 `run_hook_with`,另外帶事件裡的 Agent 名稱;`HERDR_BIN_PATH` 指向假 herdr。
        fn run_hook_full(&self, status: &str, agent: Option<&str>, socket: Option<&str>) -> i32 {
            let herdr = self.home.path().join("herdr.cmd");
            std::fs::write(&herdr, FAKE_HERDR).unwrap();
            Command::new("powershell")
                .env_remove("HERDR_SOCKET_PATH")
                .envs(socket.map(|socket| ("HERDR_SOCKET_PATH", socket)))
                .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(plugin_directory().join("notify.ps1"))
                .env("USERPROFILE", self.home.path())
                .env("HERDR_PLUGIN_EVENT", "pane.agent_status_changed")
                .env("HERDR_PLUGIN_EVENT_JSON", event_with(status, agent))
                .env("HERDR_PLUGIN_CONTEXT_JSON", CONTEXT)
                .env("HERDR_BIN_PATH", herdr)
                .status()
                .unwrap()
                .code()
                .unwrap_or(-1)
        }
    }

    fn key_a() -> String {
        "ab".repeat(32)
    }

    fn key_b() -> String {
        "cd".repeat(32)
    }

    fn write(host: &Host, name: &str, contents: &str) {
        std::fs::write(host.home.path().join(name), contents).unwrap();
    }

    /// 有 Tab label:`tab` 用它;不帶終端機標題等對話內容
    #[test]
    fn tab_label_is_sent_when_the_tab_has_one() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        write(&host, "herdr-pane-reply", PANE_REPLY);
        write(&host, "herdr-tab-reply", &tab_reply("後台 API"));

        assert_eq!(host.run_hook_full("blocked", Some("claude"), None), 0);

        let bodies = backend.bodies();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["tab"], "後台 API");
        assert!(!bodies[0].to_string().contains("secret-plan"));
    }

    /// 沒有 Tab label(查不到、空字串、或 herdr 預設的順序號)用 Agent 名稱;事件沒帶 agent 時用 pane get 的 agent
    #[test]
    fn missing_or_numeric_tab_label_falls_back_to_agent_name() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        write(&host, "herdr-pane-reply", PANE_REPLY);

        // tab get 失敗。
        host.run_hook_full("blocked", Some("claude"), None);
        // 空字串。
        write(&host, "herdr-tab-reply", &tab_reply(""));
        host.run_hook_full("blocked", Some("claude"), None);
        // 順序號。
        write(&host, "herdr-tab-reply", &tab_reply("2"));
        host.run_hook_full("blocked", Some("claude"), None);
        // 事件沒帶 agent。
        host.run_hook_full("blocked", None, None);

        let tabs: Vec<Value> = backend.bodies().iter().map(|body| body["tab"].clone()).collect();
        assert_eq!(tabs, ["claude", "claude", "claude", "claude"].map(Value::from));
    }

    /// Tab label 與 Agent 名稱都沒有:省略 `tab`(其餘欄位照送)
    #[test]
    fn tab_is_omitted_when_there_is_no_label_and_no_agent() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);

        assert_eq!(host.run_hook_full("blocked", None, None), 0);

        let bodies = backend.bodies();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].get("tab").is_none(), "{}", bodies[0]);
        assert_eq!(bodies[0]["workspace"], "weather-app");
    }

    /// 後台的 `tab` 最長 100 字元、全空白視同沒帶:以 UTF-16 單位計 100 截斷(不切開代理對)、前後空白去掉、空白 label 改用 Agent 名稱
    #[test]
    fn tab_is_cut_to_100_utf16_units_and_blank_labels_fall_back() {
        let cases = [
            ("天".repeat(150), "天".repeat(100)),
            // UTF-16 每組 3 個單位:33 組 99 個單位 + é = 100,不切開下一個代理對。
            ("é🙂".repeat(80), format!("{}é", "é🙂".repeat(33))),
            // 🙂 跨過第 100 個單位:整個捨棄，不留半個代理對。
            (format!("{}🙂tail", "a".repeat(99)), "a".repeat(99)),
            (format!("{}🙂", "a".repeat(98)), format!("{}🙂", "a".repeat(98))),
            // 截斷後尾端剩空白要再去掉。
            (format!("{} 🙂tail", "a".repeat(98)), "a".repeat(98)),
            ("  後台 API  ".to_owned(), "後台 API".to_owned()),
            ("   ".to_owned(), "claude".to_owned()),
        ];
        for (label, expected) in cases {
            let backend = Backend::start(HashMap::new());
            let host = Host::new();
            host.pair("a1", &key_a(), &backend.endpoint);
            write(&host, "herdr-pane-reply", PANE_REPLY);
            write(&host, "herdr-tab-reply", &tab_reply(&label));

            host.run_hook_full("blocked", Some("claude"), None);

            assert_eq!(backend.bodies()[0]["tab"], expected.as_str(), "{label}");
        }
    }

    fn write_lease(host: &Host, socket: &str, updated: u64) {
        let directory = host.home.path().join(".purrdr").join("host-service");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("push-sender"), format!("purrdr-lease 1\nsocket={socket}\nupdated={updated}\n")).unwrap();
    }

    fn unix_now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    /// 主機服務持有新鮮、同一個 socket 的 lease(只差大小寫與斜線方向):外掛讓位，不呼叫後台
    #[test]
    fn fresh_lease_for_the_same_socket_makes_the_plugin_yield() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        write_lease(&host, r"C:\Users\me\AppData\Roaming\herdr\herdr.sock", unix_now());

        assert_eq!(host.run_hook_with("blocked", Some("c:/users/ME/appdata/roaming/herdr/herdr.sock")), 0);

        assert!(backend.keys().is_empty());
    }

    /// lease 過期、socket 不同、沒有 HERDR_SOCKET_PATH:照送
    #[test]
    fn stale_other_or_unverifiable_lease_does_not_stop_the_plugin() {
        let socket = r"C:\herdr\herdr.sock";
        let cases: [(u64, &str, Option<&str>); 3] = [
            (unix_now() - 121, socket, Some(socket)),
            (unix_now(), r"C:\herdr\sessions\other\herdr.sock", Some(socket)),
            (unix_now(), socket, None),
        ];
        for (updated, leased, environment) in cases {
            let backend = Backend::start(HashMap::new());
            let host = Host::new();
            host.pair("a1", &key_a(), &backend.endpoint);
            write_lease(&host, leased, updated);

            host.run_hook_with("blocked", environment);

            assert_eq!(backend.keys(), [key_a()], "{updated} {leased} {environment:?}");
        }
    }

    /// 後台回 410(配對已撤銷):刪掉那支裝置的配對資料夾，其他裝置照送
    #[test]
    fn revoked_pairing_is_removed() {
        let backend = Backend::start(HashMap::from([(key_a(), 410)]));
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        host.pair("b2", &key_b(), &backend.endpoint);

        assert_eq!(host.run_hook(), 0);

        assert_eq!(backend.keys(), [key_a(), key_b()]);
        assert!(!host.pairing_directory("a1").exists());
        assert!(host.pairing_directory("b2").exists());
    }

    /// 送出後才重新配對(新密鑰)時，舊密鑰的 410 不刪新的配對
    #[test]
    fn revoked_old_key_keeps_a_pairing_renewed_during_the_call() {
        let host = Host::new();
        let renewed = host.pairing_directory("a1").join("pairing");
        let backend = Backend::start_renewing(
            HashMap::from([(key_a(), 410)]),
            Some((renewed, format!("purrdr-push 1\nendpoint=http://127.0.0.1:9/v1/push\nkey={}\n", key_b()))),
        );
        host.pair("a1", &key_a(), &backend.endpoint);

        host.run_hook();

        assert!(host.pairing_directory("a1").exists());
    }

    /// 後台其他錯誤(暫時性)不刪配對
    #[test]
    fn other_backend_errors_keep_pairing() {
        let backend = Backend::start(HashMap::from([(key_a(), 500), (key_b(), 404)]));
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        host.pair("b2", &key_b(), &backend.endpoint);

        host.run_hook();

        assert_eq!(backend.keys(), [key_a(), key_b()]);
        assert!(host.pairing_directory("a1").exists());
        assert!(host.pairing_directory("b2").exists());
    }

    /// 完成時同樣送出(狀態 done),後台回 410 一樣刪掉配對
    #[test]
    fn done_notifies_backend_and_removes_revoked_pairing() {
        let backend = Backend::start(HashMap::from([(key_a(), 410)]));
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        host.pair("b2", &key_b(), &backend.endpoint);

        assert_eq!(host.run_hook_with_status("done"), 0);

        assert_eq!(backend.keys(), [key_a(), key_b()]);
        assert_eq!(backend.statuses(), ["done", "done"]);
        assert!(!host.pairing_directory("a1").exists());
        assert!(host.pairing_directory("b2").exists());
    }

    /// 其他狀態(工作中、閒置)不送
    #[test]
    fn other_statuses_do_nothing() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);

        for status in ["working", "idle", "unknown"] {
            assert_eq!(host.run_hook_with_status(status), 0, "{status}");
        }

        assert!(backend.keys().is_empty());
    }

    // MARK: - 完成帶工作秒數(raychiutw/purrdr.host#96)與方案用量(#98),與 notify_sh 同一組情境

    fn work_file(host: &Host) -> PathBuf {
        host.home.path().join(".purrdr").join("plugin-state").join("work-w2_p1")
    }

    fn seed_start(host: &Host, seconds_ago: u64) {
        let path = work_file(host);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("working_since={}\n", unix_now() - seconds_ago)).unwrap();
    }

    fn seed_usage(host: &Host, five_hour: &str, seven_day: &str) {
        let directory = host.home.path().join(".purrdr").join("claude-usage");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("usage.json"),
            format!(
                "{{\"captured_at\":{},\"status_line_input\":{{\"session_id\":\"s\",\"rate_limits\":{{\"five_hour\":{five_hour},\"seven_day\":{seven_day}}}}}}}\n",
                unix_now()
            ),
        )
        .unwrap();
    }

    fn period(percentage: &str, resets_at: u64) -> String {
        format!("{{\"used_percentage\":{percentage},\"resets_at\":{resets_at}}}")
    }

    fn idle_period() -> String {
        period("10", unix_now() + 3 * 3600)
    }

    /// working 只記開始時間(不是 push-*),不呼叫後台;之後 done 帶 workSeconds 並清掉紀錄
    #[test]
    fn done_after_working_carries_work_seconds() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);

        host.run_hook_with_status("working");
        assert!(backend.bodies().is_empty());
        assert!(work_file(&host).exists());

        host.run_hook_with_status("done");

        let bodies = backend.bodies();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0]["workSeconds"].as_u64().is_some_and(|seconds| seconds <= 10), "{}", bodies[0]);
        assert!(!work_file(&host).exists());
    }

    /// 開始時間在 120 秒前:workSeconds 約 120
    #[test]
    fn done_reports_elapsed_seconds_from_the_recorded_start() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        seed_start(&host, 120);

        host.run_hook_with_status("done");

        let seconds = backend.bodies()[0]["workSeconds"].as_u64().expect("done 帶 workSeconds");
        assert!((120..=135).contains(&seconds), "{seconds}");
    }

    /// 算不出來一律省略:沒有開始時間、重複 working、時間倒退、壞掉的檔案;blocked 結束一段工作
    #[test]
    fn work_seconds_is_omitted_when_it_cannot_be_computed() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);

        // 沒有開始時間。
        host.run_hook_with_status("done");
        // 重複 working。
        host.run_hook_with_status("working");
        host.run_hook_with_status("working");
        host.run_hook_with_status("done");
        // 時間倒退。
        let path = work_file(&host);
        std::fs::write(&path, format!("working_since={}\n", unix_now() + 3600)).unwrap();
        host.run_hook_with_status("done");
        // 壞掉的檔案。
        std::fs::write(&path, "working_since=08\n").unwrap();
        host.run_hook_with_status("done");

        let bodies = backend.bodies();
        assert_eq!(bodies.len(), 4);
        assert!(bodies.iter().all(|body| body.get("workSeconds").is_none()), "{bodies:?}");

        // blocked 結束這一段,之後 working → done 只算新的一段。
        seed_start(&host, 3000);
        host.run_hook_with_status("blocked");
        assert!(!work_file(&host).exists());
    }

    /// 重複 working 寫下的 invalid 標記 24 小時後(以檔案修改時間)過期;未過期時不重寫、修改時間不變(與 notify_sh 同一組情境)
    #[test]
    fn an_invalid_marker_expires_after_a_day() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        let path = work_file(&host);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "working_since=invalid\n").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86400);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        host.run_hook_with_status("working");
        host.run_hook_with_status("done");
        let seconds = backend.bodies()[0]["workSeconds"].as_u64().expect("done 帶 workSeconds");
        assert!(seconds <= 15, "{seconds}");

        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::write(&path, "working_since=invalid\n").unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(earlier).unwrap();
        host.run_hook_with_status("working");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), earlier);
    }

    /// 用量跨過 80／95:各送一則(不帶 workspace、pane、tab),同一週期不重送
    #[test]
    fn usage_crossing_thresholds_is_sent_once_per_cycle() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        let resets = unix_now() + 3 * 3600;
        seed_usage(&host, &period("97.5", resets), &idle_period());

        host.run_hook_with_status("working");
        host.run_hook_with_status("idle");

        let bodies = backend.bodies();
        assert_eq!(bodies.len(), 2, "{bodies:?}");
        for (body, threshold) in bodies.iter().zip([80, 95]) {
            assert_eq!(body["status"], "usage");
            assert_eq!(body["agent"], "claude");
            assert_eq!(body["threshold"], threshold);
            assert_eq!(body["resetsAt"], resets);
            assert!(
                body.get("workspace").is_none() && body.get("pane").is_none() && body.get("tab").is_none(),
                "{body}"
            );
        }
    }

    /// 沒有用量檔、低於門檻、沒有重置時間、已過期:不送
    #[test]
    fn usage_is_not_sent_without_a_usable_reading() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);

        host.run_hook_with_status("working");
        seed_usage(&host, &idle_period(), &idle_period());
        host.run_hook_with_status("working");
        seed_usage(&host, "{\"used_percentage\":99}", "{\"used_percentage\":99}");
        host.run_hook_with_status("working");
        seed_usage(&host, &period("99", unix_now() - 10), &period("99", unix_now() - 10));
        host.run_hook_with_status("working");

        assert!(backend.bodies().is_empty());
    }

    /// 主機服務讓位(lease 新鮮):usage 與 done(workSeconds)都不送,但仍清掉開始時間
    #[test]
    fn fresh_lease_yields_usage_and_work_seconds() {
        let backend = Backend::start(HashMap::new());
        let host = Host::new();
        host.pair("a1", &key_a(), &backend.endpoint);
        let socket = r"C:\herdr\herdr.sock";
        write_lease(&host, socket, unix_now());
        seed_usage(&host, &period("90", unix_now() + 3600), &idle_period());
        seed_start(&host, 60);

        host.run_hook_with("done", Some(socket));

        assert!(backend.bodies().is_empty());
        assert!(!work_file(&host).exists());
    }
}
