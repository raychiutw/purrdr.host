# AGENTS.md

## 開發流程

- 不直接 commit 到 `master`:開 feature branch,push 後開 PR,CI 三平台全綠後以 **merge commit** 合併(不 squash)。
- 文件、註解、commit message 用繁體中文(台灣用語),技術名詞保留英文。
- 功能開發走 Matt Pocock 流程:`/grill-with-docs` 釐清需求(術語與決策寫回 app repo 的 `GLOSSARY.md`／`docs/adr/`)→ `/to-spec` 開 spec issue → `/to-tickets` 拆成標 `ready-for-agent` 的 ticket(以 blocked-by 標依賴)→ `/implement`(單張)或 `/implement-spec`(整份 spec,一條 integration branch)以 `/tdd` 實作 → `/code-review` → 依 `/pr` 寫 PR。Bug 先 `/diagnosing-bugs`;設計疑問先 `/prototype`;進來的 issue 用 `/triage` 分類。

## 建置與測試

需要 Rust stable,workspace 在 `host-service/`:

```sh
cd host-service
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p protocol-codegen        # 改了 protocol/host-service.schema.json 後重新產生型別
```

## 在真主機上跑暫時的主機服務

實機測試時以 ssh 前景跑一份暫時的主機服務(raychiutw/purrdr.host#88):

- 用**複製出來**的執行檔、獨立的 `--state-dir`(放暫存資料夾，名稱帶可辨識標記)與另一個埠，並覆寫 `HOME`／`USERPROFILE` 指到暫存資料夾。不要直接執行 `~/.purrdr/host-service/bin/` 那份：它會被當成安裝好的主機服務而啟動自動更新，可能停掉正式服務;不覆寫家目錄則會動到共用的 `~/.purrdr/push-sender`。
- **Windows:殺掉本機 ssh 不會結束遠端程序。** Win32-OpenSSH 在沒有 pty 的 session 異常結束時不結束子程序樹，主機服務也收不到 stdin EOF,會一直佔著埠。測試結束一定要明確清理。
  - 用 `purrdr-host-service-background.exe` 時有**看門狗**(0.18.0 起):`<state-dir>/running.json` 的 `pid` 是**子程序(服務)**,不是看門狗;`<state-dir>/watchdog.json` 的 `pid` 才是看門狗。只殺 `running.json` 的 pid 會在約 5 秒後被看門狗拉起。要先寫 `stop-requested` 標記(看門狗重啟前會檢查),再結束兩個程序,最後以指令列標記掃一次(含 Git Bash 的 `bash.exe` 外殼):

    ```powershell
    $state = "$env:TEMP\purrdr-test-<標記>\state"
    New-Item -ItemType File -Force "$state\stop-requested" | Out-Null   # 先要看門狗別重啟
    foreach ($file in "watchdog.json", "running.json") { if (Test-Path "$state\$file") { Stop-Process -Id (Get-Content "$state\$file" | ConvertFrom-Json).pid -Force -ErrorAction SilentlyContinue } }
    Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*purrdr-test-<標記>*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    ```

    (先殺看門狗再殺子程序也行;子程序在 Job Object 裡,看門狗一結束它就跟著結束。)
  - 暫時主機服務若用 `purrdr-host-service.exe`(非 background)則**沒有看門狗**,只需結束 `running.json` 記的 pid 並掃指令列標記。
- `ssh -tt`(配置 pty)時關閉連線會連帶結束遠端程序，但 DefaultShell 是 Git Bash 時遠端指令的參數會被吃掉，不當成標準做法。
- 只結束自己啟動、指令列含自己標記的程序;不碰正式服務(預設埠 47820、`~/.purrdr/host-service/`)。

## 發行

- 升 `host-service/service/Cargo.toml` 的 version(`Cargo.lock` 用 `cargo update -p purrdr-host-service --offline` 同步),並在 `host-service/CHANGELOG.md` 最上面加 `## <版本>` 一段給一般使用者看的更新說明(CI 會檢查)。
- 合併進 `master` 後 Release workflow 自動建置、簽章、公證並發行，Release 頁面的「本版更新」取自那一段。
- README 只寫用途、主要功能與安裝;其餘細節放安裝說明網頁或這份文件。
