//! 資料夾瀏覽(raychiutw/purrdr.host#26):`welcome.home_directory` 與 `directory.list`。
//! 只回子資料夾名稱;排除清單、主機服務資料夾與 symlink 目標都要擋。以真正的 WebSocket client 連上主機服務，
//! 測試資料放在 cargo 的 target 暫存區(不碰真正的家目錄)。

mod support;

use std::path::{Path, PathBuf};

use purrdr_host_service::transcript::TranscriptEnvironment;
use serde_json::{Value, json};
use support::{FakeHerdr, FileServiceOptions, TestClient, TestHostService, recorded, start_file_service};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    home: PathBuf,
    state: PathBuf,
    _herdr: FakeHerdr,
    service: TestHostService,
}

impl Fixture {
    async fn start() -> Fixture {
        let root = tempfile::tempdir_in(support::test_files_directory()).unwrap();
        // Windows 的 canonicalize 回 `\\?\C:\…`;app 送的是一般寫法。
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
                state_directory: Some(state.clone()),
            },
        )
        .await;
        Fixture {
            _root: root,
            home,
            state,
            _herdr: herdr,
            service,
        }
    }

    async fn client(&self) -> TestClient {
        let mut client = TestClient::connect(&self.service).await;
        let welcome = client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;
        assert_eq!(welcome["type"], "welcome");
        client
    }

    fn folder(&self, relative: &str) -> PathBuf {
        let folder = self.home.join(relative);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }
}

async fn list(client: &mut TestClient, path: &Path) -> Value {
    list_text(client, &path.to_string_lossy()).await
}

async fn list_text(client: &mut TestClient, path: &str) -> Value {
    client.request(json!({"type": "directory.list", "request_id": "d1", "path": path})).await
}

fn names(reply: &Value) -> Vec<String> {
    assert_eq!(reply["type"], "directory.list.result", "{reply}");
    reply["directories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn welcome_reports_the_home_directory() {
    let fixture = Fixture::start().await;
    let mut client = TestClient::connect(&fixture.service).await;

    let welcome = client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;

    assert_eq!(welcome["home_directory"], fixture.home.to_string_lossy().as_ref(), "{welcome}");
}

#[tokio::test]
async fn lists_only_subfolder_names_hidden_included_sorted_case_insensitively() {
    let fixture = Fixture::start().await;
    let parent = fixture.folder("Projects");
    for name in ["beta", "Alpha", ".hidden", "Zulu"] {
        fixture.folder(&format!("Projects/{name}"));
    }
    std::fs::write(parent.join("notes.txt"), "secret contents").unwrap();
    let mut client = fixture.client().await;

    let reply = list(&mut client, &parent).await;

    assert_eq!(names(&reply), [".hidden", "Alpha", "beta", "Zulu"]);
    assert_eq!(reply["path"], parent.to_string_lossy().as_ref());
    assert_eq!(reply["truncated"], false);
    assert!(!reply.to_string().contains("notes.txt"), "{reply}");
}

#[tokio::test]
async fn relative_or_dotted_paths_are_invalid_messages() {
    let fixture = Fixture::start().await;
    let parent = fixture.folder("Projects");
    let dotted = format!("{}/../Projects", parent.display());
    let mut client = fixture.client().await;

    for path in ["Projects", "~", "", dotted.as_str()] {
        let reply = list_text(&mut client, path).await;
        assert_eq!(reply["code"], "invalid_message", "{path}: {reply}");
    }
}

#[tokio::test]
async fn missing_folders_and_files_are_directory_not_found() {
    let fixture = Fixture::start().await;
    let file = fixture.home.join("plain.txt");
    std::fs::write(&file, "x").unwrap();
    let mut client = fixture.client().await;

    for path in [fixture.home.join("missing"), file] {
        let reply = list(&mut client, &path).await;
        assert_eq!(reply["code"], "directory_not_found", "{}: {reply}", path.display());
    }
}

#[tokio::test]
async fn excluded_locations_are_refused_and_excluded_children_omitted() {
    let fixture = Fixture::start().await;
    fixture.folder(".ssh/keys");
    for name in [".aws", ".config/gcloud", "Documents"] {
        fixture.folder(name);
    }
    let mut client = fixture.client().await;

    for refused in [".ssh", ".ssh/keys", ".config/gcloud"] {
        let reply = list(&mut client, &fixture.home.join(refused)).await;
        assert_eq!(reply["code"], "directory_not_allowed", "{refused}: {reply}");
    }
    let reply = list(&mut client, &fixture.home).await;
    let listed = names(&reply);
    assert!(listed.contains(&"Documents".to_owned()), "{listed:?}");
    for hidden in [".ssh", ".aws"] {
        assert!(!listed.contains(&hidden.to_owned()), "{hidden} 不該列出:{listed:?}");
    }
    let config = names(&list(&mut client, &fixture.home.join(".config")).await);
    assert!(!config.contains(&"gcloud".to_owned()), "{config:?}");
}

#[tokio::test]
async fn the_host_service_state_folder_is_refused_and_omitted() {
    let fixture = Fixture::start().await;
    fixture.folder("Documents");
    let mut client = fixture.client().await;

    let refused = list(&mut client, &fixture.state).await;
    let listed = names(&list(&mut client, &fixture.home).await);

    assert_eq!(refused["code"], "directory_not_allowed", "{refused}");
    assert_eq!(listed, ["Documents"]);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_resolved_before_checking() {
    let fixture = Fixture::start().await;
    let secrets = fixture.folder(".ssh");
    let visible = fixture.folder("Projects/real");
    let links = fixture.folder("links");
    std::os::unix::fs::symlink(&secrets, links.join("innocent")).unwrap();
    std::os::unix::fs::symlink(&visible, links.join("shortcut")).unwrap();
    std::os::unix::fs::symlink(fixture.home.join("missing"), links.join("dangling")).unwrap();
    let mut client = fixture.client().await;

    let through_link = list(&mut client, &links.join("innocent")).await;
    let listed = names(&list(&mut client, &links).await);

    assert_eq!(through_link["code"], "directory_not_allowed", "{through_link}");
    assert_eq!(listed, ["shortcut"]);
}

#[tokio::test]
async fn caps_the_number_of_entries_and_says_so() {
    let fixture = Fixture::start().await;
    let crowded = fixture.folder("crowded");
    for number in 0..1001 {
        std::fs::create_dir(crowded.join(format!("d{number:04}"))).unwrap();
    }
    let exact = fixture.folder("exact");
    for number in 0..1000 {
        std::fs::create_dir(exact.join(format!("d{number:04}"))).unwrap();
    }
    let mut client = fixture.client().await;

    let capped = list(&mut client, &crowded).await;
    let full = list(&mut client, &exact).await;

    assert_eq!(names(&capped).len(), 1000);
    assert_eq!(capped["truncated"], true);
    assert_eq!(names(&capped).first().unwrap(), "d0000");
    assert_eq!(names(&full).len(), 1000);
    assert_eq!(full["truncated"], false);
}

#[tokio::test]
async fn read_only_devices_can_list() {
    let fixture = Fixture::start().await;
    fixture.folder("Documents");
    let device = fixture.service.pair_device("借出的 iPad", true);
    let mut client = TestClient::connect_with_token(&fixture.service, Some(&device.device_token)).await;
    client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;

    let reply = list(&mut client, &fixture.home).await;

    assert_eq!(names(&reply), ["Documents"]);
}

#[cfg(unix)]
#[tokio::test]
async fn unreadable_folders_are_directory_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::start().await;
    let locked = fixture.folder("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable_anyway = std::fs::read_dir(&locked).is_ok();
    let mut client = fixture.client().await;

    let reply = list(&mut client, &locked).await;

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        return; // 以 root 執行時擋不住，略過
    }
    assert_eq!(reply["code"], "directory_unreadable", "{reply}");
}
