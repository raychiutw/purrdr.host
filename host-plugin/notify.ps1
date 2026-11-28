# Purrdr 主機外掛(Windows,best-effort):herdr 在 pane.agent_status_changed 時以新程序執行這個腳本。
# herdr 的 plugin 在 Windows 仍是 preview,這個腳本沒有在 Windows 主機上實測。
#
# 行為與 notify.sh 相同:Agent 進入 blocked 或 done 時，對 %USERPROFILE%\.purrdr\push-*\pairing 的每一支配對裝置
# 呼叫 Purrdr 後台的 /v1/push,只帶 workspace 名稱、狀態、pane id 與選用的 tab,不帶任何對話內容;後台回 410 就刪掉該配對。
# tab 的規則(主機服務直接送時也一樣):有 Tab label 用它，沒有就用該 Pane 的 Agent 名稱，都沒有就省略這個欄位。
# herdr 給外掛的 context 沒有 Tab label,所以問 herdr:`herdr pane get <pane>` 取得 tab id 與 Agent 名稱，`herdr tab get <tab>` 取得 Tab label;
# 第一支可用的配對裝置才問，整個事件只問一次;失敗就當作沒有。herdr 沒命名的 Tab 預設叫順序號(`1`、`2`…),視為沒有 Tab label。
# 檔案帶 UTF-8 BOM:Windows PowerShell 5.1 讀沒有 BOM 的檔案會用系統字碼頁，中文註解會讓整支腳本解析失敗。
#
# 0.2.0 起讓位給主機服務(raychiutw/purrdr.host#48):主機服務自己送 blocked／done 時會維護
# %USERPROFILE%\.purrdr\host-service\push-sender(purrdr-lease 1;socket=<它觀察的 herdr 路徑>;updated=<Unix 秒>,約每 30 秒更新)。
# lease 新鮮(120 秒內)且 socket 與 herdr 給這支外掛的 HERDR_SOCKET_PATH 相同，這個事件就不送，避免同一則通知兩次。
# 任何一項無法確認都照送:寧可重複也不漏推。假設(尚未在真的 Windows herdr 上驗證):兩邊的路徑字串
# 只差大小寫與斜線方向，所以比對前先統一成小寫、反斜線。
$ErrorActionPreference = 'Stop'

try {
    $event = $env:HERDR_PLUGIN_EVENT_JSON | ConvertFrom-Json
} catch {
    exit 0
}
$status = [string]$event.data.agent_status

# 0.3.0 起(raychiutw/purrdr.host#96、#98;與 notify.sh 逐段對應,規則見 notify.sh 檔頭):
# - 完成帶 workSeconds:每個 pane 進入 working 時把開始時間(Unix 秒)寫到 %USERPROFILE%\.purrdr\plugin-state\work-<pane>(不以 push- 開頭),
#   done 時讀出、算秒數並刪掉;算不出來(沒有開始時間、重複 working、時間倒退、檔案壞掉、超過 24 小時前的殘留)一律省略欄位。blocked 結束一段工作。
# - 方案用量:每次狀態改變讀 %USERPROFILE%\.purrdr\claude-usage\usage.json,five_hour／seven_day 的 used_percentage 跨過 80／95 就各送一則
#   usage(不帶 workspace、pane、tab);沒有用量檔或沒有 resets_at(或已過期)就不送;plugin-state\usage-sent 記該週期該門檻已送過。
# - lease 新鮮時 usage 與 workSeconds 也讓位(整個事件不送),但開始時間照記、done 照清。
$stateDir = Join-Path $env:USERPROFILE '.purrdr\plugin-state'
$pane = [string]$event.data.pane_id
if ($pane -notmatch '^[A-Za-z0-9:_.-]{1,64}$') { $pane = $null }

# 原子寫入狀態檔(UTF-8 無 BOM);失敗略過(狀態只是輔助,不影響推播)。
function Write-State([string]$path, [string]$contents) {
    try {
        New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
        $temporary = "$path.$PID"
        [System.IO.File]::WriteAllText($temporary, $contents + "`n", (New-Object System.Text.UTF8Encoding($false)))
        Move-Item -LiteralPath $temporary -Destination $path -Force
    } catch {}
}

# 讀 working_since= 的值:只接受不帶前導零的 1–12 位數字,其餘回 $null。
function Read-WorkStart([string]$path) {
    try {
        $line = Get-Content -LiteralPath $path -ErrorAction Stop | Where-Object { $_ -like 'working_since=*' } | Select-Object -First 1
        $value = [string]($line -replace '^working_since=', '')
        if ($value -match '^[1-9][0-9]{0,11}$') { return [long]$value }
    } catch {}
    return $null
}

# 每個 pane 的工作開始時間。done 時算出的秒數放 $workSeconds(算不出來是 $null)。
$workSeconds = $null
if ($pane) {
    $workFile = Join-Path $stateDir ('work-' + $pane.Replace(':', '_'))
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    if ($status -ceq 'working') {
        $stale = $false
        if (Test-Path -LiteralPath $workFile -PathType Leaf) {
            $since = Read-WorkStart $workFile
            if ($null -ne $since -and ($now - $since) -gt 86400) { $stale = $true }
        }
        if ((Test-Path -LiteralPath $workFile -PathType Leaf) -and -not $stale) {
            # 上一段還沒結束又收到 working(重複或亂序):這一段的開始時間不可信,作廢(留到 done 或 blocked 才清)。
            Write-State $workFile 'working_since=invalid'
        } else {
            Write-State $workFile "working_since=$now"
        }
    } elseif ($status -ceq 'done') {
        if (Test-Path -LiteralPath $workFile -PathType Leaf) {
            $since = Read-WorkStart $workFile
            if ($null -ne $since) {
                $elapsed = $now - $since
                if ($elapsed -ge 0 -and $elapsed -le 86400) { $workSeconds = $elapsed }
            }
            Remove-Item -LiteralPath $workFile -Force -ErrorAction SilentlyContinue
        }
    } else {
        Remove-Item -LiteralPath $workFile -Force -ErrorAction SilentlyContinue
    }
}

function ConvertTo-ComparableSocket([string]$path) {
    return $path.Replace('/', '\').TrimEnd('\').ToLowerInvariant()
}

function Test-HostServiceSending {
    if (-not $env:HERDR_SOCKET_PATH) { return $false }
    $lease = Join-Path $env:USERPROFILE '.purrdr\host-service\push-sender'
    if (-not (Test-Path -LiteralPath $lease -PathType Leaf)) { return $false }
    try {
        $lines = Get-Content -LiteralPath $lease -ErrorAction Stop
        $socket = [string](($lines | Where-Object { $_ -like 'socket=*' } | Select-Object -First 1) -replace '^socket=', '')
        $updated = [string](($lines | Where-Object { $_ -like 'updated=*' } | Select-Object -First 1) -replace '^updated=', '')
        if (-not $socket -or $updated -notmatch '^[0-9]{1,15}$') { return $false }
        $age = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - [long]$updated
        if ($age -lt -5 -or $age -gt 120) { return $false }
        return (ConvertTo-ComparableSocket $socket) -ceq (ConvertTo-ComparableSocket $env:HERDR_SOCKET_PATH)
    } catch {
        return $false
    }
}

if (Test-HostServiceSending) { exit 0 }

# 對一支配對裝置送一則請求,回 HTTP 狀態碼(成功 200;連不上等沒有回應回 0)。
# 後台回 410(裝置已被遠端關閉、配對撤銷)就刪掉那支裝置的配對資料夾;送出期間重新配對了(密鑰換了)就不刪。
function Send-Push($directory, [string]$file, [string]$key, [string]$endpoint, $body) {
    try {
        Invoke-RestMethod -Method Post -Uri $endpoint -ContentType 'application/json' -TimeoutSec 10 `
            -Body ([System.Text.Encoding]::UTF8.GetBytes(($body | ConvertTo-Json -Compress))) | Out-Null
        return 200
    } catch {
        $response = $_.Exception.Response
        $current = (Get-Content -LiteralPath $file -ErrorAction SilentlyContinue | Where-Object { $_ -like 'key=*' } | Select-Object -First 1) -replace '^key=', ''
        if ($response -and [int]$response.StatusCode -eq 410 -and $current -eq $key) {
            Remove-Item -LiteralPath $directory.FullName -Recurse -Force -ErrorAction SilentlyContinue
        }
        if ($response) { return [int]$response.StatusCode }
        return 0
    }
}

# 讀 JSON 數值(只收真正的數字,字串不算),取整數部分;不是數字回 $null。
function Get-WholeNumber($value) {
    if ($value -is [int] -or $value -is [long] -or $value -is [double] -or $value -is [decimal]) {
        if ($value -ge 0 -and $value -lt 1e12) { return [long][math]::Floor([double]$value) }
    }
    return $null
}

# 方案用量(#98):跨過 80／95 且這個週期還沒送過的門檻,逐一送給每支配對裝置。
function Invoke-UsageCheck {
    $usageFile = Join-Path $env:USERPROFILE '.purrdr\claude-usage\usage.json'
    if (-not (Test-Path -LiteralPath $usageFile -PathType Leaf)) { return }
    try {
        $limits = (Get-Content -LiteralPath $usageFile -Raw -ErrorAction Stop | ConvertFrom-Json).status_line_input.rate_limits
    } catch { return }
    if (-not $limits) { return }
    $now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $sentFile = Join-Path $stateDir 'usage-sent'
    $sent = @()
    if (Test-Path -LiteralPath $sentFile -PathType Leaf) {
        $sent = @(Get-Content -LiteralPath $sentFile -ErrorAction SilentlyContinue | Where-Object { $_ -match '^[0-9]{1,12}:[0-9]+$' })
    }
    $pending = @()
    foreach ($period in 'five_hour', 'seven_day') {
        $used = Get-WholeNumber $limits.$period.used_percentage
        $resets = Get-WholeNumber $limits.$period.resets_at
        if ($null -eq $used -or $null -eq $resets) { continue }
        # 已過重置時間:那個視窗已經重置,檔案裡的比例過時了。
        if ($resets -le $now) { continue }
        foreach ($threshold in 80, 95) {
            if ($used -lt $threshold) { continue }
            if ($sent -contains "${resets}:${threshold}") { continue }
            $pending += , @($resets, $threshold)
        }
    }
    # 已過期週期的紀錄清掉。
    $kept = @($sent | Where-Object { [long]($_ -split ':')[0] -gt $now })
    if ($kept.Count -ne $sent.Count) { Write-State $sentFile ($kept -join "`n") }
    if ($pending.Count -eq 0) { return }
    $delivered = @()
    $paired = Join-Path $env:USERPROFILE '.purrdr'
    if (-not (Test-Path -LiteralPath $paired)) { return }
    foreach ($directory in Get-ChildItem -LiteralPath $paired -Directory -Filter 'push-*') {
        $file = Join-Path $directory.FullName 'pairing'
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { continue }
        $lines = Get-Content -LiteralPath $file
        $key = ($lines | Where-Object { $_ -like 'key=*' } | Select-Object -First 1) -replace '^key=', ''
        $endpoint = ($lines | Where-Object { $_ -like 'endpoint=*' } | Select-Object -First 1) -replace '^endpoint=', ''
        if (-not $key -or -not $endpoint) { continue }
        foreach ($item in $pending) {
            $body = [ordered]@{ key = $key; status = 'usage'; agent = 'claude'; threshold = [int]$item[1]; resetsAt = [long]$item[0] }
            $code = Send-Push $directory $file $key $endpoint $body
            if ($code -ge 200 -and $code -lt 300) { $delivered += "$($item[0]):$($item[1])" }
            if ($code -eq 410) { break }
        }
    }
    foreach ($item in $delivered) {
        if ($sent -notcontains $item) { $sent += $item }
    }
    if ($delivered.Count -gt 0) { Write-State $sentFile ($sent -join "`n") }
}

try { Invoke-UsageCheck } catch {}

# 往下是 blocked／done 的推播;其他狀態(working、idle…)到此為止。
if ($status -cne 'blocked' -and $status -cne 'done') { exit 0 }

$workspace = $null
try {
    $context = $env:HERDR_PLUGIN_CONTEXT_JSON | ConvertFrom-Json
    $workspace = [string]$context.workspace_label
} catch {}
if ([string]::IsNullOrEmpty($workspace)) { $workspace = [string]$event.data.workspace_id }
if ([string]::IsNullOrEmpty($workspace)) { exit 0 }

$herdr = if ($env:HERDR_BIN_PATH) { $env:HERDR_BIN_PATH } else { 'herdr' }
$session = $null
$tab = $null
$agent = [string]$event.data.agent
$asked = $false

$root = Join-Path $env:USERPROFILE '.purrdr'
if (-not (Test-Path -LiteralPath $root)) { exit 0 }

foreach ($directory in Get-ChildItem -LiteralPath $root -Directory -Filter 'push-*') {
    $file = Join-Path $directory.FullName 'pairing'
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { continue }
    $lines = Get-Content -LiteralPath $file
    $key = ($lines | Where-Object { $_ -like 'key=*' } | Select-Object -First 1) -replace '^key=', ''
    $endpoint = ($lines | Where-Object { $_ -like 'endpoint=*' } | Select-Object -First 1) -replace '^endpoint=', ''
    if (-not $key -or -not $endpoint) { continue }

    if (-not $asked) {
        $asked = $true
        $tabId = ''
        if ($pane) {
            try {
                $info = & $herdr pane get $pane 2>$null | ConvertFrom-Json
                $session = [string]$info.result.pane.agent_session.value
                if (-not $agent) { $agent = [string]$info.result.pane.agent }
                $tabId = [string]$info.result.pane.tab_id
            } catch {}
        }
        if ($tabId -match '^[A-Za-z0-9:_.-]{1,64}$') {
            try {
                $tab = [string]((& $herdr tab get $tabId 2>$null | ConvertFrom-Json).result.tab.label)
            } catch {}
        }
        $tab = ([string]$tab).Trim()
        $agent = ([string]$agent).Trim()
        # 順序號(herdr 預設的 Tab 名稱)或空字串(含全空白)不是 Tab label。
        if ($tab -match '^[0-9]*$') { $tab = $null }
        if (-not $tab) { $tab = $agent }
        # 後台的 tab 最長 100 字元。規則順序(與主機服務、notify.sh 相同):前面已修頭尾空白 →
        # 最多 100 個 UTF-16 單位(.NET 的 String.Length 就是 UTF-16 單位，不管後台怎麼數字元都不會超過) →
        # 第 100 個單位是代理對的高位就整對捨棄(不切開) → 截斷後再修尾端空白。
        if ($tab -and $tab.Length -gt 100) {
            $tab = $tab.Substring(0, 100)
            if ([char]::IsHighSurrogate($tab[99])) { $tab = $tab.Substring(0, 99) }
            $tab = $tab.TrimEnd()
        }
    }

    $muted = @($lines | Where-Object { $_ -like 'muted=*' } | ForEach-Object { $_.Substring(6) })
    if ($muted.Count -gt 0 -and $session -and ($muted -contains $session)) { continue }

    $body = [ordered]@{ key = $key; workspace = $workspace; status = $status }
    if ($pane) { $body.pane = $pane }
    if ($tab) { $body.tab = $tab }
    # 完成才帶工作秒數(算不出來就省略)。
    if ($status -ceq 'done' -and $null -ne $workSeconds) { $body.workSeconds = [long]$workSeconds }
    # 410:裝置已被遠端關閉(配對撤銷),Send-Push 刪掉配對之後不再呼叫後台。其他失敗不重試。
    Send-Push $directory $file $key $endpoint $body | Out-Null
}
