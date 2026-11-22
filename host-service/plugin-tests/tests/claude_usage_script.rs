//! Claude 方案用量的 status line 包裝(repo 根目錄的 `claude-usage/`,raychiutw/purrdr.host#64):
//! 每次寫 `sessions/<session_id>.json`、含 `rate_limits` 時寫 `usage.json`,再把輸入原樣交給原本的 status line。
//!
//! 在暫存家目錄真的執行腳本:POSIX 版(`statusline.sh`)在 macOS／Linux,PowerShell 版(`statusline.ps1`)在 Windows。
//! 原本的 status line 是 `original-command` 裡的一行指令：把收到的輸入存起來，再印出 `STATUS`。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

fn scripts_directory() -> PathBuf {
    std::env::var_os("PURRDR_CLAUDE_USAGE_DIR").map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../claude-usage"), PathBuf::from)
}

/// 安裝好的樣子：`<家目錄>/.purrdr/claude-usage/`(Windows 的腳本也放這裡，以 `$PSScriptRoot` 找資料夾)。
struct Installed {
    home: tempfile::TempDir,
}

impl Installed {
    fn new(with_original: bool) -> Installed {
        let home = tempfile::tempdir().unwrap();
        let installed = Installed { home };
        std::fs::create_dir_all(installed.dir()).unwrap();
        if cfg!(windows) {
            std::fs::copy(scripts_directory().join("statusline.ps1"), installed.dir().join("statusline.ps1")).unwrap();
        }
        if with_original {
            std::fs::write(installed.dir().join("original-command"), "cat > received.txt; printf STATUS").unwrap();
        }
        installed
    }

    fn dir(&self) -> PathBuf {
        self.home.path().join(".purrdr").join("claude-usage")
    }

    fn run(&self, input: &str) -> Output {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("powershell");
            command
                .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(self.dir().join("statusline.ps1"));
            command
        } else {
            let mut command = Command::new("sh");
            command.arg(scripts_directory().join("statusline.sh"));
            command
        };
        let mut child = command
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .current_dir(self.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    fn json(&self, relative: &str) -> Option<Value> {
        std::fs::read_to_string(self.dir().join(relative))
            .ok()
            .map(|text| serde_json::from_str(&text).unwrap())
    }

    fn session_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir().join("sessions"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn input(session: &str, rate_limits: bool) -> String {
    let mut value =
        json!({"session_id": session, "model": {"id": "claude-opus-5-5"}, "context_window": {"used_percentage": 40, "context_window_size": 200000}});
    if rate_limits {
        value["rate_limits"] = json!({"five_hour": {"used_percentage": 12, "resets_at": 1795000000}});
    }
    value.to_string()
}

fn assert_passed_through(installed: &Installed, output: &Output, sent: &str) {
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "STATUS", "{output:?}");
    let received = std::fs::read_to_string(installed.home.path().join("received.txt")).unwrap();
    assert_eq!(received.trim_end(), sent, "原本的 status line 收到同一份輸入");
}

#[test]
fn input_with_rate_limits_saves_usage_and_the_session_then_runs_the_original() {
    let installed = Installed::new(true);
    let sent = input("abc-1", true);

    let output = installed.run(&sent);

    assert_passed_through(&installed, &output, &sent);
    let sent_value: Value = serde_json::from_str(&sent).unwrap();
    let usage = installed.json("usage.json").expect("有 rate_limits 時寫 usage.json");
    assert!(usage["captured_at"].as_u64().is_some(), "{usage}");
    assert_eq!(usage["status_line_input"], sent_value);
    let session = installed.json("sessions/abc-1.json").expect("每次都寫 session 檔");
    assert!(session["captured_at"].as_u64().is_some(), "{session}");
    assert_eq!(session["session_id"], "abc-1");
    assert_eq!(session["status_line_input"], sent_value);
}

#[test]
fn input_without_rate_limits_still_saves_the_session() {
    let installed = Installed::new(true);
    let sent = input("abc-1", false);

    let output = installed.run(&sent);

    assert_passed_through(&installed, &output, &sent);
    assert!(installed.json("usage.json").is_none(), "沒有 rate_limits 時不寫 usage.json");
    assert_eq!(installed.json("sessions/abc-1.json").unwrap()["session_id"], "abc-1");
}

#[test]
fn each_session_has_its_own_file() {
    let installed = Installed::new(false);

    installed.run(&input("first-session", false));
    installed.run(&input("second.session_2", true));

    assert_eq!(installed.session_files(), ["first-session.json", "second.session_2.json"]);
}

#[test]
fn sessions_not_updated_for_seven_days_are_removed() {
    let installed = Installed::new(false);
    let sessions = installed.dir().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let day = Duration::from_secs(24 * 60 * 60);
    for (name, age) in [("old.json", day * 8), ("recent.json", day * 6)] {
        let file = std::fs::File::create(sessions.join(name)).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    installed.run(&input("abc-1", false));

    assert_eq!(installed.session_files(), ["abc-1.json", "recent.json"]);
}

#[test]
fn unsafe_session_ids_are_not_written() {
    let installed = Installed::new(false);

    for session in ["../escape", "..", "a/b", "a\\\\b", ""] {
        installed.run(&input(session, false));
    }

    assert!(installed.session_files().is_empty(), "{:?}", installed.session_files());
    assert!(!installed.home.path().join(".purrdr").join("escape.json").exists());
}

#[test]
fn without_an_original_status_line_it_prints_nothing_and_succeeds() {
    let installed = Installed::new(false);

    let output = installed.run(&input("abc-1", true));

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

/// 存不了檔(資料夾唯讀、sessions 被同名檔案占住)時照樣交給原本的 status line。
#[cfg(unix)]
#[test]
fn saving_failures_do_not_break_the_status_line() {
    use std::os::unix::fs::PermissionsExt;
    let installed = Installed::new(true);
    std::fs::write(installed.dir().join("sessions"), "not a folder").unwrap();
    std::fs::set_permissions(installed.dir(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let sent = input("abc-1", true);

    let output = installed.run(&sent);

    std::fs::set_permissions(installed.dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_passed_through(&installed, &output, &sent);
    assert!(output.stderr.is_empty(), "不輸出錯誤:{}", String::from_utf8_lossy(&output.stderr));
    assert!(installed.json("usage.json").is_none());
}
