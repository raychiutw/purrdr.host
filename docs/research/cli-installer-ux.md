# 互動式 CLI 安裝程式 UX 研究

調查日期：2026-11-23。只引用官方指南與安裝程式本身的原始碼／文件;每項結論都附來源。

## 建議摘要

- **預設值就要是大多數人該選的那個**,而且每一題都能用 flag 回答;可以問，但不能「非問不可」。([clig.dev — Arguments and flags](https://clig.dev/#arguments-and-flags))
- **只有 stdin 是 TTY 才問**;有 `--no-input`(或 `-y`/`--yes`)就完全不問。([clig.dev — Interactivity](https://clig.dev/#interactivity))
- **`curl | sh` 時改從 `/dev/tty` 讀**;連 `/dev/tty` 都沒有就失敗並告訴使用者怎麼改用非互動模式，例如 rustup 的「Unable to run interactively. Run with -y to accept defaults, --help for additional options」。([rustup-init.sh](https://raw.githubusercontent.com/rust-lang/rustup/master/rustup-init.sh))
- **把 `CI` 環境變數和 stdin 不是 TTY 都視為非互動**,並且印一行說明是因為哪個條件才切成非互動。([Homebrew install.sh](https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh))
- **動手前先列出摘要**(會裝到哪裡、會改哪些設定),然後讓使用者選「1) 照預設 2) 自訂 3) 取消」,直接按 Enter 就是 1。([rustup common.rs](https://raw.githubusercontent.com/rust-lang/rustup/master/src/cli/common.rs)、[Homebrew install.sh](https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh))
- **提示要把預設值寫出來**:`問題 [預設]`、`(Y/n)`;空白輸入就套用預設值。([rustup common.rs](https://raw.githubusercontent.com/rust-lang/rustup/master/src/cli/common.rs))
- **用環境變數帶參數，兩個平台都適用**(`*_INSTALL_DIR`、`*_NO_MODIFY_PATH`);shell 用 `| env X=… sh` 或 `sh -s -- <flags>`,PowerShell 用 `$env:X=…; irm … | iex`,或用 `& {$(irm …)} -Param` 傳參數。([uv installer](https://docs.astral.sh/uv/reference/installer/)、[Bun](https://bun.com/docs/installation))
- **結束時告訴使用者狀態改了什麼，並提示下一步可以跑的指令**。([clig.dev — Output](https://clig.dev/#output)、[Homebrew「Next steps」](https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh))
- **讓 AI agent 也看得懂**:提供 `--json`、在文件列出所有環境變數、提供 `llms.txt` 與 agent guide;偵測 `CLAUDECODE=1` 這類訊號。([clig.dev](https://clig.dev/#output)、[llmstxt.org](https://llmstxt.org/)、[herdr agent guide](https://herdr.dev/agent-guide.md)、[Claude Code env vars](https://code.claude.com/docs/en/env-vars))

## 逐一來源筆記

### clig.dev(Command Line Interface Guidelines)
- Interactivity:「Only use prompts or interactive elements if `stdin` is an interactive terminal (a TTY)」;「If `--no-input` is passed, don't prompt」;密碼輸入不回顯;Ctrl-C 一定要能中斷。([#interactivity](https://clig.dev/#interactivity))
- Arguments and flags:「Make the default the right thing for most users」;「Never require a prompt. Always provide a way of passing input with flags or arguments」。標準 flag 有 `-f/--force`、`--json`、`-q/--quiet`。清單裡**沒有** `-y/--yes`,不過 rustup 與 tailscale 都在用。危險操作在互動模式下要求輸入 y/yes,非互動模式則要 `--force`。([#arguments-and-flags](https://clig.dev/#arguments-and-flags))
- Output:「Humans come first, machines second」;`--json` 輸出 JSON;stdout 不是 TTY 就不放動畫;遵守 `NO_COLOR`;「If you change state, tell the user」;「Suggest commands the user should run」。([#output](https://clig.dev/#output))
- Help:`-h/--help`,先放範例，常用 flag 排前面。([#help](https://clig.dev/#help))
- 環境變數：名稱只用大寫、數字和底線;**不要從環境變數讀 secret**。([#environment-variables](https://clig.dev/#environment-variables))
- 設定優先序:flag > shell 環境變數 > 專案設定 > 使用者設定 > 系統設定。([#configuration](https://clig.dev/#configuration))
- 盡早驗證輸入，在造成任何影響前就停下來;100ms 內要有輸出。([#robustness-guidelines](https://clig.dev/#robustness-guidelines))

### rustup
- `rustup-init.sh`:沒帶 `-y` 而且 `[ ! -t 0 ]`(stdin 是管線)時，會用 `< /dev/tty` 執行 rustup-init;stdout 也不是終端機時就報錯，請使用者改用 `-y`。`-y/--yes` 會設 `need_tty=no`。所有參數用 `"$@"` 原樣轉交給 binary。另外有 `RUSTUP_UPDATE_ROOT`、`RUSTUP_VERSION` 等環境變數。([rustup-init.sh](https://raw.githubusercontent.com/rust-lang/rustup/master/rustup-init.sh))
- 選單有三個選項:「1) Proceed with standard installation (default - just press enter)」,自訂過之後改成「Proceed with selected options」;「2) Customize installation」;「3) Cancel installation」。`question_str` 顯示 `{question} [{default}]`,`question_bool` 顯示 `(Y/n)`/`(y/N)`,空白輸入就套用預設值。([common.rs](https://raw.githubusercontent.com/rust-lang/rustup/master/src/cli/common.rs))
- 自訂流程先說明「You may simply press the Enter key to leave unchanged」,逐題問完後再列出「Current installation options:」,回到選單。安裝前的訊息會列出安裝路徑，以及 PATH 會寫進哪些 rc 檔。`-y` 會略過這段訊息、選單，以及可略過的檢查。([self_update.rs](https://raw.githubusercontent.com/rust-lang/rustup/master/src/cli/self_update.rs))
- 官方文件用 `sh -s -- --no-modify-path`、`--default-toolchain`、`--profile` 示範怎麼透過管線傳參數。([rustup other installation methods](https://rust-lang.github.io/rustup/installation/other.html))

### Homebrew install.sh
- 以下三種情況會切成非互動，每種都印一行說明原因：設了 `NONINTERACTIVE`、設了 `CI`、stdin 不是 TTY(`! -t 0`)。在 CI 裡強制互動會被拒絕(「Cannot run force-interactive mode in CI」)。([install.sh](https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh))
- 先印「This script will install:」並列出路徑，再顯示「Press RETURN/ENTER to continue or any other key to abort」;非互動模式不會停下來等。要用 sudo 之前會先說明「which may request your password」。最後印出「Next steps:」。(同上)

### uv / Deno / Bun / Volta
- uv:`UV_INSTALL_DIR`、`UV_NO_MODIFY_PATH`、`UV_UNMANAGED_INSTALL`;用法是 `curl … | env UV_NO_MODIFY_PATH=1 sh`、`| sh -s -- --help`;Windows 用 `powershell -ExecutionPolicy ByPass -c {$env:UV_INSTALL_DIR = "…";irm … | iex}`。([uv installer](https://docs.astral.sh/uv/reference/installer/))
- Deno:用 `DENO_INSTALL` 改安裝目錄。([Deno installation](https://docs.deno.com/runtime/getting_started/installation/))
- Bun:shell 用 `bash -s "bun-v1.3.3"`;PowerShell 用 `iex "& {$(irm https://bun.com/install.ps1)} -Version 1.3.3"`,把下載的腳本包成 scriptblock 傳參數。([Bun installation](https://bun.com/docs/installation))
- Volta:用 `VOLTA_HOME` 指定位置，`--skip-setup` 不修改 profile,`--version` 指定版本;略過 setup 時會明確印出「No changes were made to user profile settings」。([volta-install.sh](https://raw.githubusercontent.com/volta-cli/volta/main/dev/unix/volta-install.sh))

### gh auth login / tailscale
- `gh auth login` 不帶參數就進入互動流程;每個選項都有對應 flag(`--hostname`、`--git-protocol`、`--web`、`--skip-ssh-key`);token 可以用 `--with-token` 從 stdin 讀，headless 環境則用 `GH_TOKEN`。([gh auth login](https://cli.github.com/manual/gh_auth_login))
- `tailscale up`:「Flags are not persisted between runs」,另有 `--reset`、`--auth-key` 和 `--json`(格式可能變動)。([tailscale up](https://tailscale.com/kb/1241/tailscale-up))
- `tailscale serve`:加 `--bg` 才會在重開機後保留;要關掉就在原指令後面加 `off`,或執行 `tailscale serve reset`;用 `tailscale serve status --json` 查狀態;`--yes` 代表「Update without interactive prompts」。([tailscale serve](https://tailscale.com/kb/1242/tailscale-serve))

### Windows:`irm | iex` 與主控台輸入
- `Invoke-Expression` 只接受一個字串，沒有轉交參數的機制;官方也提醒要謹慎使用，並建議盡量改用 call operator。所以要傳參數只有兩條路：環境變數，或 `& {…}` scriptblock。([Invoke-Expression](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.utility/invoke-expression))
- 就算 STDIN 已經被重導向，用 `CreateFile("CONIN$")` 仍然拿得到主控台輸入 handle,作用等同 Unix 的 `/dev/tty`。([Console Handles](https://learn.microsoft.com/en-us/windows/console/console-handles))
- 可以用 `Console.IsInputRedirected` 判斷 stdin 是否被重導向。([Console.IsInputRedirected](https://learn.microsoft.com/en-us/dotnet/api/system.console.isinputredirected))

### 給 AI agent 看的文件
- `llms.txt`:H1 是唯一必要的段落，接著是 blockquote 摘要，再來是用 H2 分組的連結清單，以及一段可以略過的「Optional」;每一頁另外提供 `.md` 版本。([llmstxt.org](https://llmstxt.org/))
- herdr 的 `llms.txt` 是精簡索引，連到特定 tag 的原始 Markdown,並提醒 agent 不要去點 HTML 文件頁。([herdr llms.txt](https://herdr.dev/llms.txt))
- herdr 的 agent guide 寫給「協助人類的 agent」看，內容依序是概念、安裝、首次使用、設定、診斷，最後是 agent 守則：不可以自己發明 flag 或設定鍵，要寫入設定前必須先問使用者。它用 `HERDR_ENV=1` 判斷 agent 是否已經在 herdr 裡面，並提供 `--json` 診斷指令。([herdr agent-guide.md](https://herdr.dev/agent-guide.md))
- Claude Code 會在它啟動的子程序裡設 `CLAUDECODE=1`,包括 Bash/PowerShell tool 和 hook;如果要排除 IDE 終端機，可以改看 `CLAUDE_CODE_CHILD_SESSION`。其他 agent 沒有統一的訊號，第三方偵測套件各自檢查不同的變數。([Claude Code env vars](https://code.claude.com/docs/en/env-vars))

## 套用到 purrdr 安裝程式

現況：`install.sh`／`install.ps1` 只負責下載，接著呼叫 `<binary> install "$@"`。shell 版已經會用 `</dev/tty` 重導 stdin,所以真正的提示邏輯寫在 Rust 裡。

1. **每題都要有 flag 和環境變數**:例如 `--port`／`PURRDR_PORT`、`--https-port`、`--herdr-session`、`--auto-update/--no-auto-update`、`--no-modify-path`。優先序依照 clig:flag > 環境變數 > 既有設定 > 預設值。
2. **判斷是否非互動**:符合以下任一條件就照預設值安裝，不會停下來等輸入。條件是：`--yes`/`-y`、`PURRDR_NONINTERACTIVE`、`CI`、`CLAUDECODE`,或者 stdin 不是 TTY 而且也開不了 `/dev/tty`/`CONIN$`。然後印一行說明是因為哪個條件才切成非互動，以及怎麼覆寫。如果某題沒有安全的預設值，例如 port 被占用，就直接失敗，並在錯誤訊息裡給出要補的 flag(仿照 rustup 的錯誤訊息)。
3. **Windows 對齊 Unix 的作法**:Rust 端在 stdin 被重導向時改開 `CONIN$`;`install.ps1` 的文件要寫出 `$env:PURRDR_…=…; irm … | iex`,以及 `& ([scriptblock]::Create((irm …))) -Yes` 這兩種用法。
4. **rustup 式的摘要選單**:先列出「將要做的事」,包括 binary 路徑、服務註冊方式、local port、`tailscale serve --bg --https=<port>`、herdr session、auto-update、PATH 會寫進哪些檔案。再給「1) 照這樣安裝(Enter) 2) 自訂 3) 取消」,自訂完回到同一份摘要。
5. **提示格式**:`本機 port [<預設值>]:`、`自動更新?(Y/n)`。輸入當下就驗證，例如 port 範圍與是否被占用，錯了就重問，不要等到最後才報錯。
6. **結尾印出「之後要怎麼改」**:列出改設定的指令、`tailscale serve status`、關閉時要跑的 `tailscale serve --https=<port> off`、解除安裝的指令、怎麼確認服務正在跑;如果沒有修改 PATH,也要明講。
7. **給 agent 看的**:安裝結束時，如果設了 `--json` 或偵測到 agent,就輸出一段機器可讀的結果(安裝路徑、port、URL、設定檔位置);在 docs 網站提供 `llms.txt` 與 `agent-guide.md`,列出所有 flag 和環境變數，並寫明「agent 不可以自行假設 port 或 session,必須先問使用者，或改用 `--yes` 接受預設值」。
