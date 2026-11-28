//! Claude 的 slash 清單(raychiutw/purrdr.host#106):`slash.list`。
//! 只讀家目錄(`files.home`)底下的 `.claude`;以真正的 WebSocket client 連上主機服務，
//! 測試資料放在暫存家目錄(不碰真正的 ~/.claude)。

mod support;

use std::path::{Path, PathBuf};

use purrdr_host_service::transcript::TranscriptEnvironment;
use serde_json::{Value, json};
use support::{FakeHerdr, FileServiceOptions, TestClient, TestHostService, recorded, start_file_service};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    base: PathBuf,
    home: PathBuf,
    _herdr: FakeHerdr,
    service: TestHostService,
}

impl Fixture {
    async fn start() -> Fixture {
        let root = tempfile::tempdir_in(support::test_files_directory()).unwrap();
        let canonical = std::fs::canonicalize(root.path()).unwrap().to_string_lossy().into_owned();
        let base = PathBuf::from(canonical.strip_prefix(r"\\?\").unwrap_or(&canonical));
        let home = base.join("home");
        let state = home.join("host-state");
        std::fs::create_dir_all(&state).unwrap();
        let herdr = FakeHerdr::start(vec![("ping", recorded("ping.ndjson"))]).await;
        let service = start_file_service(
            herdr.socket.clone(),
            FileServiceOptions {
                transcripts: TranscriptEnvironment {
                    home: Some(home.clone()),
                    ..TranscriptEnvironment::default()
                },
                temporary_directory: None,
                state_directory: Some(state),
            },
        )
        .await;
        Fixture {
            _root: root,
            base,
            home,
            _herdr: herdr,
            service,
        }
    }

    /// 寫檔(相對家目錄),必要時建立資料夾。
    fn write(&self, relative: &str, contents: &str) {
        write_at(&self.home.join(relative), contents);
    }

    async fn slash_list(&self) -> Value {
        let mut client = TestClient::connect(&self.service).await;
        let welcome = client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;
        assert_eq!(welcome["type"], "welcome");
        let reply = client.request(json!({"type": "slash.list", "request_id": "s1"})).await;
        assert_eq!(reply["type"], "slash.list.result", "{reply}");
        assert_eq!(reply["request_id"], "s1");
        reply
    }
}

fn write_at(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn names(reply: &Value) -> Vec<String> {
    reply["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["name"].as_str().unwrap().to_owned())
        .collect()
}

fn command<'a>(reply: &'a Value, name: &str) -> &'a Value {
    reply["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["name"] == name)
        .unwrap_or_else(|| panic!("沒有 {name}:{reply}"))
}

/// 安裝一個 plugin:installed_plugins.json 的一筆(`user` 層)與它的 plugin.json。
fn install_plugin(fixture: &Fixture, key: &str, install: &str, manifest: &str) {
    let path = fixture.home.join(install);
    let entry = json!({"scope": "user", "installPath": path.to_string_lossy(), "version": "1.0.0"});
    let installed = fixture.home.join(".claude/plugins/installed_plugins.json");
    let mut document: Value = std::fs::read_to_string(&installed)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(json!({"version": 2, "plugins": {}}));
    document["plugins"][key] = json!([entry]);
    write_at(&installed, &document.to_string());
    write_at(&path.join(".claude-plugin/plugin.json"), manifest);
}

#[tokio::test]
async fn without_a_claude_folder_the_list_is_empty() {
    let fixture = Fixture::start().await;

    let reply = fixture.slash_list().await;

    assert_eq!(reply["commands"], json!([]));
    assert_eq!(reply["disabled_plugins"], json!([]));
    assert_eq!(reply["truncated"], false);
}

#[tokio::test]
async fn user_skills_and_commands_use_frontmatter_names_hints_and_summaries() {
    let fixture = Fixture::start().await;
    fixture.write(
        ".claude/skills/release/SKILL.md",
        "---\nname: release\ndescription: >-\n  Cut a release\n  for the ledger app\nargument-hint: <version>\n---\nbody that must never be returned\n",
    );
    fixture.write(".claude/skills/folder-name/SKILL.md", "---\ndescription: 'It''s quoted'\n---\n");
    fixture.write(
        ".claude/skills/hidden/SKILL.md",
        "---\nname: hidden\nuser-invocable: false\ndescription: nope\n---\n",
    );
    fixture.write(".claude/commands/deploy.md", "---\r\ndescription: \"Deploy\\tit\"\r\n---\r\n");
    fixture.write(".claude/commands/db/migrate.md", "---\ndescription: Run the migrations\n---\n");
    fixture.write(".claude/commands/not-markdown.txt", "---\ndescription: ignored\n---\n");

    let reply = fixture.slash_list().await;

    assert_eq!(names(&reply), ["db:migrate", "deploy", "folder-name", "release"]);
    assert_eq!(command(&reply, "release")["summary"], "Cut a release for the ledger app");
    assert_eq!(command(&reply, "release")["argument_hint"], "<version>");
    assert_eq!(command(&reply, "release")["source"], "user");
    assert!(command(&reply, "release").get("plugin").is_none());
    assert_eq!(command(&reply, "folder-name")["summary"], "It's quoted");
    assert_eq!(command(&reply, "deploy")["summary"], "Deploy\tit");
    assert_eq!(command(&reply, "db:migrate")["summary"], "Run the migrations");
    assert!(command(&reply, "db:migrate").get("argument_hint").is_none());
    assert!(!reply.to_string().contains("must never be returned"), "{reply}");
}

#[tokio::test]
async fn plugin_skills_follow_declared_nested_paths_and_default_commands() {
    let fixture = Fixture::start().await;
    install_plugin(
        &fixture,
        "mattpocock-skills@mattpocock",
        ".claude/plugins/cache/mattpocock/mattpocock-skills/1.0.0",
        r#"{"name":"mattpocock-skills","skills":["./skills/engineering/tdd","./skills/productivity/"]}"#,
    );
    let root = ".claude/plugins/cache/mattpocock/mattpocock-skills/1.0.0";
    fixture.write(
        &format!("{root}/skills/engineering/tdd/SKILL.md"),
        "---\nname: tdd\ndescription: Test-driven development.\n---\n",
    );
    fixture.write(&format!("{root}/skills/engineering/undeclared/SKILL.md"), "---\nname: undeclared\n---\n");
    fixture.write(
        &format!("{root}/skills/productivity/handoff/SKILL.md"),
        "---\nname: handoff\nargument-hint: What next?\n---\n",
    );
    // 宣告了 skills 路徑，預設的 skills/ 就不再掃;commands 沒宣告，用預設 commands/。
    fixture.write(&format!("{root}/skills/stray/SKILL.md"), "---\nname: stray\n---\n");
    fixture.write(&format!("{root}/commands/review.md"), "---\ndescription: Review it\n---\n");
    fixture.write(&format!("{root}/commands/git/commit.md"), "---\ndescription: Commit\n---\n");
    fixture.write(".claude/skills/mine/SKILL.md", "---\nname: mine\n---\n");

    let reply = fixture.slash_list().await;

    assert_eq!(
        names(&reply),
        [
            "mine",
            "mattpocock-skills:git:commit",
            "mattpocock-skills:handoff",
            "mattpocock-skills:review",
            "mattpocock-skills:tdd"
        ]
    );
    let tdd = command(&reply, "mattpocock-skills:tdd");
    assert_eq!(tdd["source"], "plugin");
    assert_eq!(tdd["plugin"], "mattpocock-skills");
    assert_eq!(tdd["summary"], "Test-driven development.");
    assert_eq!(command(&reply, "mattpocock-skills:handoff")["argument_hint"], "What next?");
    assert_eq!(reply["disabled_plugins"], json!([]));
}

#[tokio::test]
async fn disabled_plugins_are_reported_without_their_commands() {
    let fixture = Fixture::start().await;
    install_plugin(
        &fixture,
        "frontend-design@official",
        ".claude/plugins/cache/official/frontend-design/1",
        r#"{"name":"frontend-design"}"#,
    );
    fixture.write(
        ".claude/plugins/cache/official/frontend-design/1/skills/frontend-design/SKILL.md",
        "---\nname: frontend-design\n---\n",
    );
    install_plugin(
        &fixture,
        "superpowers@official",
        ".claude/plugins/cache/official/superpowers/1",
        r#"{"name":"superpowers"}"#,
    );
    fixture.write(
        ".claude/plugins/cache/official/superpowers/1/skills/brainstorming/SKILL.md",
        "---\nname: brainstorming\n---\n",
    );
    fixture.write(
        ".claude/settings.json",
        r#"{"enabledPlugins":{"frontend-design@official":false,"superpowers@official":true}}"#,
    );

    let reply = fixture.slash_list().await;

    assert_eq!(names(&reply), ["superpowers:brainstorming"]);
    assert_eq!(reply["disabled_plugins"], json!(["frontend-design"]));
}

#[tokio::test]
async fn entries_that_escape_through_dotdot_or_symlinks_are_not_listed() {
    let fixture = Fixture::start().await;
    let outside = fixture.base.join("outside");
    write_at(&outside.join("stolen/SKILL.md"), "---\nname: stolen\n---\n");
    write_at(&outside.join("plugin/skills/leaked/SKILL.md"), "---\nname: leaked\n---\n");
    write_at(&outside.join("cmds/leaked-command.md"), "---\ndescription: x\n---\n");
    #[cfg(unix)]
    {
        std::fs::create_dir_all(fixture.home.join(".claude/skills")).unwrap();
        std::os::unix::fs::symlink(outside.join("stolen"), fixture.home.join(".claude/skills/stolen")).unwrap();
        std::fs::create_dir_all(fixture.home.join(".claude/commands")).unwrap();
        std::os::unix::fs::symlink(outside.join("cmds"), fixture.home.join(".claude/commands/linked")).unwrap();
    }
    // plugin 安裝在家目錄之外。
    let installed = json!({"version": 2, "plugins": {"leaky@m": [{"scope": "user", "installPath": outside.join("plugin").to_string_lossy()}]}});
    write_at(&fixture.home.join(".claude/plugins/installed_plugins.json"), &installed.to_string());
    // plugin 在家目錄內，但宣告的路徑用 .. 跳出去。
    install_plugin(
        &fixture,
        "dotdot@m",
        ".claude/plugins/cache/dotdot",
        &json!({"name": "dotdot", "skills": ["../../../../../outside"], "commands": ["./../../../../../outside/cmds"]}).to_string(),
    );
    fixture.write(".claude/skills/safe/SKILL.md", "---\nname: safe\n---\n");

    let reply = fixture.slash_list().await;

    let listed = names(&reply);
    assert!(listed.contains(&"safe".to_owned()), "{reply}");
    for leaked in [
        "stolen",
        "leaked",
        "leaked-command",
        "linked:leaked-command",
        "leaky:leaked",
        "dotdot:leaked",
        "dotdot:stolen",
    ] {
        assert!(!listed.contains(&leaked.to_owned()), "{leaked}: {reply}");
    }
    assert_eq!(listed, ["safe"], "{reply}");
}

#[tokio::test]
async fn malformed_json_files_give_an_empty_plugin_part_not_an_error() {
    let fixture = Fixture::start().await;
    fixture.write(".claude/plugins/installed_plugins.json", "{ not json");
    fixture.write(".claude/settings.json", "[1,2");
    fixture.write(".claude/skills/ok/SKILL.md", "---\nname: ok\n---\n");

    let reply = fixture.slash_list().await;

    assert_eq!(names(&reply), ["ok"]);
}
