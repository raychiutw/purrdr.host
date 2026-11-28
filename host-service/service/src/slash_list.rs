//! Claude 的 slash 清單(raychiutw/purrdr.host#106):`slash.list` 讀家目錄(`files.home`)底下的 `.claude`,
//! 規則比照 app 的 SSH 作法(`ClaudeSlashDiscovery`):使用者層的 skills／commands,加上已安裝且啟用的 plugin
//! (`plugin.json` 宣告了 skills／commands 路徑就只用宣告的,沒宣告才用預設的 `skills/`、`commands/`)。
//!
//! 只讀檔案、不執行任何指令。每個檔案先解析 symlink,實際位置必須在家目錄(或解析後的 `.claude`)之內、
//! 不在排除清單、不在主機服務資料夾;plugin 宣告的路徑不可含 `..`。只讀 frontmatter 開頭的一小段，
//! 且只留名稱、說明與參數提示,絕不回傳檔案內容。大小與數量都有上限。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::host_files;
use crate::protocol::SlashCommandEntry;

/// 回傳的指令數上限;更多時以 `truncated` 告知。
pub const MAXIMUM_COMMANDS: usize = 2000;
/// 最多看幾個目錄項目就停(限制工作量，不只是回覆大小)。
const MAXIMUM_SCANNED_ENTRIES: usize = 20_000;
/// 指令資料夾的遞迴深度上限(也擋 symlink 造成的迴圈)。
const MAXIMUM_DEPTH: usize = 8;
/// 最多處理幾個已安裝的 plugin。
const MAXIMUM_PLUGINS: usize = 200;
/// JSON 設定檔(installed_plugins.json、settings.json、plugin.json)的大小上限。
const MAXIMUM_JSON_BYTES: u64 = 1024 * 1024;
/// 每個 SKILL.md／指令檔只讀開頭這麼多 bytes(frontmatter 在最前面)。
const MAXIMUM_FRONTMATTER_BYTES: u64 = 16 * 1024;
const MAXIMUM_SUMMARY_CHARS: usize = 500;
const MAXIMUM_HINT_CHARS: usize = 200;

#[derive(Debug, Default, PartialEq)]
pub struct Listing {
    pub commands: Vec<SlashCommandEntry>,
    pub disabled_plugins: Vec<String>,
    pub truncated: bool,
}

/// `home` 是主機服務的家目錄;`claude_config_dir` 是 `CLAUDE_CONFIG_DIR`(沒設就用 `<home>/.claude`,與對話紀錄的解析相同);
/// `state_directory` 是主機服務資料夾(一律不讀)。沒有家目錄或沒有設定目錄回空清單。
pub fn list(home: Option<&Path>, claude_config_dir: Option<&Path>, state_directory: &Path) -> Listing {
    list_with_scan_limit(home, claude_config_dir, state_directory, MAXIMUM_SCANNED_ENTRIES)
}

fn list_with_scan_limit(home: Option<&Path>, claude_config_dir: Option<&Path>, state_directory: &Path, scan_limit: usize) -> Listing {
    let Some(home) = home.and_then(host_files::resolve) else {
        return Listing::default();
    };
    let claude = claude_config_dir.map_or_else(|| home.join(".claude"), Path::to_owned);
    let Some(claude) = host_files::resolve(&claude).filter(|resolved| resolved.is_dir()) else {
        return Listing::default();
    };
    let state = host_files::resolve(state_directory).unwrap_or_else(|| state_directory.to_owned());
    let mut scanner = Scanner {
        roots: vec![home, claude.clone()],
        state,
        remaining: scan_limit,
        truncated: false,
        found: Vec::new(),
    };

    scanner.skills(&claude.join("skills"), &Origin::User);
    scanner.commands(&claude.join("commands"), &Origin::User);
    let disabled_plugins = scanner.plugins(&claude);
    scanner.finish(disabled_plugins)
}

#[derive(Clone)]
enum Origin {
    User,
    Plugin(String),
}

struct Scanner {
    /// 實際路徑必須落在其中之一(家目錄與解析後的 `.claude`:使用者可能把 `.claude` 連到 dotfiles 資料夾)。
    roots: Vec<PathBuf>,
    state: PathBuf,
    remaining: usize,
    truncated: bool,
    found: Vec<(Origin, SlashCommandEntry)>,
}

impl Scanner {
    /// 解析 symlink 後的實際路徑;不在允許範圍就是 `None`。
    fn allowed(&self, path: &Path) -> Option<PathBuf> {
        let resolved = host_files::resolve(path)?;
        let inside = self.roots.iter().any(|root| host_files::is_under_or_same(&resolved, root));
        (inside && !host_files::is_excluded(&resolved) && !host_files::is_under_or_same(&resolved, &self.state)).then_some(resolved)
    }

    /// 花一個掃描額度;用完就標示不完整。
    fn spend(&mut self) -> bool {
        if self.remaining == 0 {
            self.truncated = true;
            return false;
        }
        self.remaining -= 1;
        true
    }

    fn read_json(&self, path: &Path) -> Option<Value> {
        let resolved = self.allowed(path)?;
        let file = File::open(&resolved).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAXIMUM_JSON_BYTES {
            return None;
        }
        let mut text = String::new();
        file.take(MAXIMUM_JSON_BYTES).read_to_string(&mut text).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// 檔案開頭的 frontmatter(不含 `---`),每行一個元素。不是一般檔案、不在允許範圍或沒有 frontmatter 回 `None`。
    fn read_frontmatter(&self, path: &Path) -> Option<Vec<String>> {
        let resolved = self.allowed(path)?;
        let file = File::open(&resolved).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut head = Vec::new();
        file.take(MAXIMUM_FRONTMATTER_BYTES).read_to_end(&mut head).ok()?;
        let text = String::from_utf8_lossy(&head);
        // 有些編輯器會在檔頭寫 UTF-8 BOM,不該讓 frontmatter 被當成不存在。
        let mut lines = text.strip_prefix('\u{feff}').unwrap_or(&text).lines();
        if lines.next()?.trim_end() != "---" {
            return None;
        }
        Some(lines.take_while(|line| line.trim_end() != "---").map(str::to_owned).collect())
    }

    /// 一個資料夾直接含 `SKILL.md` 就是一個 skill;否則它的每個子資料夾各自可以有 `SKILL.md`(一層)。
    fn skills(&mut self, root: &Path, origin: &Origin) {
        let Some(resolved) = self.allowed(root) else {
            return;
        };
        // plugin.json 直接指到一個 `SKILL.md`:那個檔案所在的資料夾就是一個 skill。
        if resolved.is_file() {
            if root.file_name().is_some_and(|name| name == "SKILL.md")
                && let Some(folder) = root.parent()
            {
                self.add_skill(folder, origin);
            }
            return;
        }
        if !resolved.is_dir() {
            return;
        }
        if resolved.join("SKILL.md").is_file() {
            self.add_skill(root, origin);
            return;
        }
        for name in self.child_names(&resolved) {
            self.add_skill(&root.join(&name), origin);
        }
    }

    fn add_skill(&mut self, folder: &Path, origin: &Origin) {
        let Some(lines) = self.read_frontmatter(&folder.join("SKILL.md")) else {
            return;
        };
        let meta = parse_frontmatter(&lines);
        let folder_name = folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let local = meta.get("name").filter(|name| !name.is_empty()).cloned().unwrap_or(folder_name);
        self.add(origin, local, &meta);
    }

    /// `root` 底下所有 `.md`(遞迴、跟著 symlink);名稱是相對 `root` 的路徑，`/` 換成 `:`。
    fn commands(&mut self, root: &Path, origin: &Origin) {
        // plugin.json 直接指到一個 `.md` 檔:它就是一個指令,名稱取檔名(去掉 `.md`)。
        if self.allowed(root).is_some_and(|resolved| resolved.is_file()) {
            if let Some(parent) = root.parent()
                && let Some(local) = command_name(parent, root)
                && let Some(lines) = self.read_frontmatter(root)
            {
                self.add(origin, local, &parse_frontmatter(&lines));
            }
            return;
        }
        let mut visited = HashSet::new();
        self.walk_commands(root, root, origin, 0, &mut visited);
    }

    fn walk_commands(&mut self, root: &Path, folder: &Path, origin: &Origin, depth: usize, visited: &mut HashSet<PathBuf>) {
        let Some(resolved) = self.allowed(folder).filter(|resolved| resolved.is_dir()) else {
            return;
        };
        if depth > MAXIMUM_DEPTH || !visited.insert(resolved.clone()) {
            return;
        }
        for name in self.child_names(&resolved) {
            let child = folder.join(&name);
            let Some(actual) = self.allowed(&child) else { continue };
            if actual.is_dir() {
                self.walk_commands(root, &child, origin, depth + 1, visited);
            } else if actual.is_file()
                && let Some(local) = command_name(root, &child)
                && let Some(lines) = self.read_frontmatter(&child)
            {
                self.add(origin, local, &parse_frontmatter(&lines));
            }
        }
    }

    /// 資料夾的子項目名稱(排序;每個項目花一個掃描額度)。
    fn child_names(&mut self, resolved: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(resolved) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            if !self.spend() {
                break;
            }
            if let Ok(name) = entry.file_name().into_string() {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    fn add(&mut self, origin: &Origin, local: String, meta: &HashMap<String, String>) {
        if meta.get("user-invocable").is_some_and(|value| value.eq_ignore_ascii_case("false")) {
            return;
        }
        let (name, source, plugin) = match origin {
            Origin::User => (local, "user", None),
            Origin::Plugin(plugin) => (format!("{plugin}:{local}"), "plugin", Some(plugin.clone())),
        };
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_alphanumeric() || matches!(character, '_' | ':' | '.' | '-'))
        {
            return;
        }
        let limited = |text: &str, limit: usize| text.chars().take(limit).collect::<String>();
        self.found.push((
            origin.clone(),
            SlashCommandEntry {
                name,
                argument_hint: meta
                    .get("argument-hint")
                    .filter(|hint| !hint.is_empty())
                    .map(|hint| limited(hint, MAXIMUM_HINT_CHARS)),
                summary: limited(meta.get("description").map_or("", String::as_str), MAXIMUM_SUMMARY_CHARS),
                source: source.to_owned(),
                plugin,
            },
        ));
    }

    /// 已安裝且啟用的 plugin;回傳停用的 plugin 名稱。
    fn plugins(&mut self, claude: &Path) -> Vec<String> {
        let installed = self.read_json(&claude.join("plugins/installed_plugins.json"));
        let Some(plugins) = installed.as_ref().and_then(|document| document.get("plugins")).and_then(Value::as_object) else {
            return Vec::new();
        };
        // settings.json 與 settings.local.json 的 enabledPlugins 合併,local 覆蓋同名的鍵。
        let mut enabled = serde_json::Map::new();
        for file in ["settings.json", "settings.local.json"] {
            if let Some(layer) = self
                .read_json(&claude.join(file))
                .and_then(|document| document.get("enabledPlugins").and_then(Value::as_object).cloned())
            {
                enabled.extend(layer);
            }
        }
        let mut keys: Vec<&String> = plugins.keys().collect();
        keys.sort();
        if keys.len() > MAXIMUM_PLUGINS {
            keys.truncate(MAXIMUM_PLUGINS);
            self.truncated = true;
        }

        let mut disabled = BTreeSet::new();
        for key in keys {
            let fallback = key.split('@').next().unwrap_or(key).to_owned();
            if enabled.get(key).and_then(Value::as_bool) == Some(false) {
                disabled.insert(fallback);
                continue;
            }
            // 沒有 cwd,只認使用者層(沒有 projectPath)的那一筆。
            let entries: Vec<&Value> = match &plugins[key] {
                Value::Array(many) => many.iter().collect(),
                one @ Value::Object(_) => vec![one],
                _ => Vec::new(),
            };
            let Some(install) = entries
                .into_iter()
                .find(|entry| entry.get("projectPath").is_none())
                .and_then(|entry| entry.get("installPath"))
                .and_then(Value::as_str)
                .filter(|path| host_files::is_plain_absolute(path))
                .and_then(|path| self.allowed(Path::new(path)))
            else {
                continue;
            };
            self.plugin(&install, fallback);
        }
        disabled.into_iter().collect()
    }

    fn plugin(&mut self, install: &Path, fallback: String) {
        let manifest = self.read_json(&install.join(".claude-plugin/plugin.json"));
        let name = manifest
            .as_ref()
            .and_then(|document| document.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map_or(fallback, str::to_owned);
        let origin = Origin::Plugin(name);
        let declared = |key: &str, default: &str| -> Vec<PathBuf> {
            let paths: Vec<&str> = match manifest.as_ref().and_then(|document| document.get(key)) {
                Some(Value::String(single)) => vec![single.as_str()],
                Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            // 宣告了就只用宣告的(不合法的路徑直接丟掉,不退回預設)。
            if paths.is_empty() {
                vec![install.join(default)]
            } else {
                paths.into_iter().filter_map(|path| plugin_relative(install, path)).collect()
            }
        };
        for path in declared("skills", "skills") {
            self.skills(&path, &origin);
        }
        for path in declared("commands", "commands") {
            self.commands(&path, &origin);
        }
    }

    fn finish(self, disabled_plugins: Vec<String>) -> Listing {
        let Scanner { mut found, truncated, .. } = self;
        // 使用者層在前;plugin 依名稱。同名只留排序在前的(使用者層優先)。
        found.sort_by(|(left_origin, left), (right_origin, right)| {
            let rank = |origin: &Origin| match origin {
                Origin::User => (0, String::new()),
                Origin::Plugin(name) => (1, name.clone()),
            };
            rank(left_origin).cmp(&rank(right_origin)).then_with(|| left.name.cmp(&right.name))
        });
        let mut seen = HashSet::new();
        let mut commands: Vec<SlashCommandEntry> = found
            .into_iter()
            .map(|(_, entry)| entry)
            .filter(|entry| seen.insert(entry.name.clone()))
            .collect();
        let over_limit = commands.len() > MAXIMUM_COMMANDS;
        commands.truncate(MAXIMUM_COMMANDS);
        Listing {
            commands,
            disabled_plugins,
            truncated: truncated || over_limit,
        }
    }
}

/// plugin 內的相對路徑(`./skills/x`)→ 絕對路徑。含 `..`、反斜線或 `:` 的不接受。
fn plugin_relative(install: &Path, path: &str) -> Option<PathBuf> {
    let mut result = install.to_owned();
    for segment in path.split('/').filter(|segment| !segment.is_empty() && *segment != ".") {
        if segment == ".." || segment.contains(['\\', ':']) || segment.chars().any(char::is_control) {
            return None;
        }
        result.push(segment);
    }
    Some(result)
}

/// 指令檔相對 `root` 的名稱:`db/migrate.md` → `db:migrate`。
fn command_name(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    let text = relative.to_str()?;
    let stem = text.strip_suffix(".md")?;
    Some(stem.split(['/', '\\']).collect::<Vec<_>>().join(":"))
}

/// SKILL.md／指令 .md 的 YAML frontmatter(只取頂層的字串欄位)。支援:引號字串、block scalar(`>`、`|` 與 `-`、`+`)、
/// 跨行的 plain scalar。不是完整的 YAML(與 app 的 `ClaudeFrontmatter` 同一套規則)。
fn parse_frontmatter(lines: &[String]) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        index += 1;
        let Some(first) = line.chars().next() else { continue };
        let Some(colon) = line.find(':') else { continue };
        if matches!(first, ' ' | '\t' | '#') {
            continue;
        }
        let key = line[..colon].trim().to_owned();
        let raw = line[colon + 1..].trim().to_owned();
        let mut continuation = Vec::new();
        while index < lines.len() && (lines[index].is_empty() || lines[index].starts_with([' ', '\t'])) {
            continuation.push(lines[index].trim().to_owned());
            index += 1;
        }
        let non_empty = || continuation.iter().filter(|line| !line.is_empty()).cloned();
        let value = if let Some(indicator @ ('>' | '|')) = raw.chars().next()
            && raw.chars().count() <= 3
        {
            while continuation.last().is_some_and(String::is_empty) {
                continuation.pop();
            }
            if indicator == '>' {
                continuation
                    .split(|line| line.is_empty())
                    .filter(|paragraph| !paragraph.is_empty())
                    .map(|paragraph| paragraph.join(" "))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                continuation.join("\n")
            }
        } else if raw.starts_with('"') {
            unquote_double(&std::iter::once(raw.clone()).chain(non_empty()).collect::<Vec<_>>().join(" "))
        } else if raw.starts_with('\'') {
            let joined = std::iter::once(raw.clone()).chain(non_empty()).collect::<Vec<_>>().join(" ");
            let inner = &joined[1..];
            inner.strip_suffix('\'').unwrap_or(inner).replace("''", "'")
        } else {
            std::iter::once(raw.clone())
                .chain(non_empty())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        result.insert(key, value);
    }
    result
}

fn unquote_double(text: &str) -> String {
    let mut inner = &text[1..];
    if inner.ends_with('"') && (!inner.ends_with("\\\"") || inner.ends_with("\\\\\"")) {
        inner = &inner[..inner.len() - 1];
    }
    let mut output = String::new();
    let mut escaping = false;
    for character in inner.chars() {
        if escaping {
            output.push(match character {
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            escaping = false;
        } else if character == '\\' {
            escaping = true;
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_command_count_is_capped_and_reported_as_truncated() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        for index in 0..(MAXIMUM_COMMANDS + 5) {
            write(&home.path().join(format!(".claude/commands/c{index:05}.md")), "---\ndescription: x\n---\n");
        }

        let listing = list_with_scan_limit(Some(home.path()), None, state.path(), usize::MAX);

        assert_eq!(listing.commands.len(), MAXIMUM_COMMANDS);
        assert!(listing.truncated);
    }

    #[test]
    fn scanning_stops_at_the_scan_limit_and_reports_truncated() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        for index in 0..20 {
            write(&home.path().join(format!(".claude/commands/c{index:02}.md")), "---\ndescription: x\n---\n");
        }

        let listing = list_with_scan_limit(Some(home.path()), None, state.path(), 5);

        assert_eq!(listing.commands.len(), 5, "只掃前 5 個項目");
        assert!(listing.truncated);
        assert!(!list_with_scan_limit(Some(home.path()), None, state.path(), 100).truncated);
    }

    #[test]
    fn only_the_head_of_a_huge_file_is_read_and_long_fields_are_cut() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let long = "x".repeat(5000);
        write(
            &home.path().join(".claude/commands/long.md"),
            &format!("---\ndescription: {long}\nargument-hint: {long}\n---\n{}", "y".repeat(2_000_000)),
        );
        // frontmatter 沒結束在讀取範圍內:仍只回有限長度。
        write(
            &home.path().join(".claude/commands/open.md"),
            &format!("---\ndescription: ok\n{}", "z: 1\n".repeat(100_000)),
        );

        let listing = list_with_scan_limit(Some(home.path()), None, state.path(), usize::MAX);

        let long = listing.commands.iter().find(|command| command.name == "long").unwrap();
        assert_eq!(long.summary.chars().count(), MAXIMUM_SUMMARY_CHARS);
        assert_eq!(long.argument_hint.as_ref().unwrap().chars().count(), MAXIMUM_HINT_CHARS);
        assert!(listing.commands.iter().any(|command| command.name == "open"));
    }

    #[test]
    fn a_symlink_loop_in_commands_terminates() {
        #[cfg(unix)]
        {
            let home = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            write(&home.path().join(".claude/commands/a/one.md"), "---\ndescription: x\n---\n");
            std::os::unix::fs::symlink(home.path().join(".claude/commands"), home.path().join(".claude/commands/a/back")).unwrap();

            let listing = list_with_scan_limit(Some(home.path()), None, state.path(), usize::MAX);

            assert_eq!(listing.commands.iter().map(|command| command.name.as_str()).collect::<Vec<_>>(), ["a:one"]);
        }
    }

    #[test]
    fn frontmatter_block_scalars_fold_paragraphs() {
        let lines: Vec<String> = ">-\n  one\n  two\n\n  three".lines().map(str::to_owned).collect();
        let parsed = parse_frontmatter(
            &[format!("description: {}", lines[0])]
                .into_iter()
                .chain(lines[1..].iter().cloned())
                .collect::<Vec<_>>(),
        );

        assert_eq!(parsed["description"], "one two\nthree");
    }
}
