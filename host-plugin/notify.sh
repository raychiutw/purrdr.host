#!/bin/sh
# Purrdr 主機外掛(macOS／Linux):herdr 在 pane.agent_status_changed 時以新程序執行這個腳本。
#
# Agent 進入 blocked(「需要你」)或 done(「完成」)時，對 ~/.purrdr/push-*/pairing 的每一支配對裝置呼叫 Purrdr 後台的 /v1/push;
# 是否真的通知由後台依裝置偏好決定。
# 請求只帶 workspace 名稱、狀態、pane id 與選用的 tab(推播內容由後台組),不帶終端機標題或任何對話內容。
# tab 的規則(主機服務直接送時也一樣):有 Tab label 用它，沒有就用該 Pane 的 Agent 名稱，都沒有就省略這個欄位。
# herdr 給外掛的 context 沒有 Tab label(文件只列 workspace、tab、focused pane 等 id 類欄位),所以問 herdr 兩次:
# `herdr pane get <pane>` 取得該 pane 的 tab id 與 Agent 名稱,`herdr tab get <tab>` 取得 Tab label;
# 第一支可用的配對裝置才問，整個事件只問一次;失敗(herdr 不在、pane 已關閉)就當作沒有。
# 後台限制 tab 最長 100 字元、全空白視同沒帶:前後空白去掉、以字元計截斷，空白的 label 或 Agent 名稱視為沒有。
# herdr 沒命名的 Tab 預設叫順序號(`1`、`2`…),不是使用者取的名字，視為沒有 Tab label。
# 配對密鑰經 stdin 交給 curl,不出現在指令參數裡。
# 沒有常駐程序;失敗不重試。
# 後台回 410(裝置已被遠端關閉、配對撤銷)時刪掉那支裝置的配對資料夾，之後不再對它呼叫後台。
#
# 0.2.0 起讓位給主機服務(raychiutw/purrdr.host#48):主機服務自己送 blocked／done 時會維護
# ~/.purrdr/host-service/push-sender(purrdr-lease 1;socket=<它觀察的 herdr socket>;updated=<Unix 秒>,約每 30 秒更新)。
# lease 新鮮(120 秒內)且 socket 與 herdr 給這支外掛的 HERDR_SOCKET_PATH 是同一個，這個事件就不送，避免同一則通知兩次。
# 任何一項無法確認(沒有 lease、過期、壞掉、沒有 HERDR_SOCKET_PATH、socket 不同)都照送:寧可重複也不漏推。
# 假設(尚未在真的 herdr 上逐字驗證):herdr 注入的 HERDR_SOCKET_PATH 與主機服務解析出的路徑指向同一個 socket 檔;
# 兩邊都先把資料夾正規化(pwd -P)再比，所以符號連結與 .. 的寫法差異不影響。
#
# 0.3.0 起(raychiutw/purrdr.host#96、#98):
# - 完成帶 `workSeconds`(工作秒數)。外掛沒有常駐程序，herdr 事件也沒有時間戳、順序不保證，所以每個 pane 進入 working 時
#   把開始時間(Unix 秒)寫到 ~/.purrdr/plugin-state/work-<pane>(不以 `push-` 開頭，不會被當成配對資料夾),done 時讀出、算秒數並刪掉。
#   算不出來一律省略欄位(不帶 0):沒有開始時間(重啟後、done 先於 working)、重複 working(順序不可信，這一段作廢)、
#   時間倒退、檔案壞掉、開始時間超過 24 小時前(殘留)。blocked 結束一段工作(使用者介入),之後再 working 是新的一段。
#   working 與其他狀態(idle…)本身不推播。
# - 方案用量:每次狀態改變都讀 Claude 的 status line 用量檔 ~/.purrdr/claude-usage/usage.json(five_hour、seven_day 兩個視窗),
#   used_percentage 跨過 80／95 就各送一則 `{"status":"usage","agent":"claude","threshold":N,"resetsAt":<Unix 秒>}`(不帶 workspace、pane、tab)。
#   沒有用量檔或沒有 resets_at(或已過期)就不送;後台以 (agent,resetsAt,threshold) 去重,外掛另在 ~/.purrdr/plugin-state/usage-sent
#   記「該週期該門檻已送過」(每行 `<resetsAt>:<門檻>`,週期過了就清掉),至少一支裝置收到(2xx)才記。
# - 主機服務同版本會自己送這兩項，所以 lease 新鮮時 usage 與 workSeconds 也讓位(整個事件不送);但開始時間照記、done 照清。

status=$(printf '%s' "${HERDR_PLUGIN_EVENT_JSON:-}" | sed -nE 's/.*"agent_status":"([a-z_]*)".*/\1/p')

# 取出 JSON 字串欄位的值，維持 JSON 的跳脫(原樣放回請求的 JSON 裡)。
json_string() {
  printf '%s' "$1" | sed -nE 's/.*"'"$2"'":"(([^"\\]|\\.)*)".*/\1/p' | head -n 1
}

pane=$(json_string "${HERDR_PLUGIN_EVENT_JSON:-}" pane_id)
case $pane in
  '' | *[!A-Za-z0-9:_.-]*) pane= ;;
esac

state_dir="$HOME/.purrdr/plugin-state"

# 原子寫入狀態檔:$1 檔案，$2 內容。失敗略過(狀態只是輔助，不影響推播)。
write_state() {
  mkdir -p "$state_dir" 2>/dev/null || return 0
  _tmp="$1.$$"
  if printf '%s\n' "$2" > "$_tmp" 2>/dev/null; then
    mv -f "$_tmp" "$1" 2>/dev/null || rm -f "$_tmp"
  else
    rm -f "$_tmp"
  fi
}

# 讀 `working_since=` 的值:只接受不帶前導零的 1–12 位數字(其餘當作壞掉，也避免被當成八進位算術)。
read_work_start() {
  _since=$(sed -n 's/^working_since=//p' "$1" 2>/dev/null | head -n 1)
  case $_since in
    '' | 0[0-9]* | *[!0-9]*) return 1 ;;
  esac
  [ "${#_since}" -le 12 ] || return 1
  printf '%s' "$_since"
}

# 每個 pane 的工作開始時間(見檔頭說明)。done 時算出的秒數放 work_seconds(算不出來留空)。
work_seconds=
track_work() {
  [ -n "$pane" ] || return 0
  _file="$state_dir/work-$(printf '%s' "$pane" | tr ':' '_')"
  _now=$(date +%s)
  case $status in
    working)
      _stale=
      if [ -f "$_file" ]; then
        if _since=$(read_work_start "$_file"); then
          [ "$((_now - _since))" -gt 86400 ] && _stale=1
        elif [ -n "$(find "$_file" -mmin +1440 2>/dev/null)" ]; then
          # 讀不出開始時間(例如 invalid 標記):改用檔案修改時間套同樣的 24 小時過期。
          _stale=1
        fi
      fi
      if [ -f "$_file" ] && [ -z "$_stale" ]; then
        # 上一段還沒結束又收到 working(重複或亂序):這一段的開始時間不可信，作廢(留到 done 或 blocked 才清)。
        # 已經是 invalid 就不重寫,讓修改時間停在第一次作廢的時候,24 小時後才過得了期。
        grep -qx 'working_since=invalid' "$_file" 2>/dev/null || write_state "$_file" 'working_since=invalid'
      else
        write_state "$_file" "working_since=$_now"
      fi
      ;;
    done)
      if [ -f "$_file" ]; then
        if _since=$(read_work_start "$_file"); then
          _elapsed=$((_now - _since))
          if [ "$_elapsed" -ge 0 ] && [ "$_elapsed" -le 86400 ]; then work_seconds=$_elapsed; fi
        fi
        rm -f "$_file"
      fi
      ;;
    *) rm -f "$_file" ;;
  esac
}

track_work

# 路徑正規化:資料夾解析符號連結與 ..,檔名照舊(與主機服務寫 lease 時的算法相同)。
canonical_path() {
  _dir=$(cd -- "$(dirname -- "$1")" 2>/dev/null && pwd -P) || { printf '%s' "$1"; return; }
  printf '%s/%s' "$_dir" "$(basename -- "$1")"
}

# 主機服務持有新鮮、同一個 socket 的 lease 時回成功(代表這個事件由它送)。
host_service_is_sending() {
  [ -n "${HERDR_SOCKET_PATH:-}" ] || return 1
  _lease="$HOME/.purrdr/host-service/push-sender"
  [ -f "$_lease" ] || return 1
  _socket=$(sed -n 's/^socket=//p' "$_lease" | head -n 1)
  _updated=$(sed -n 's/^updated=//p' "$_lease" | head -n 1)
  [ -n "$_socket" ] || return 1
  case $_updated in '' | *[!0-9]*) return 1 ;; esac
  _age=$(($(date +%s) - _updated))
  # 允許一點時鐘誤差;時間在未來太多視為壞掉。
  [ "$_age" -ge -5 ] && [ "$_age" -le 120 ] || return 1
  [ "$(canonical_path "$_socket")" = "$(canonical_path "$HERDR_SOCKET_PATH")" ]
}

host_service_is_sending && exit 0

# 對一支配對裝置送一則請求(密鑰經 stdin):$1 pairing 檔，$2 密鑰，$3 endpoint，$4 請求本文(key 之後的欄位，不含大括號)。
# 狀態碼放 push_code。後台回 410(裝置已被遠端關閉、配對撤銷)就刪掉那支裝置的配對資料夾;送出期間重新配對了(密鑰換了)就不刪。
post_push() {
  push_code=$(printf '{"key":"%s",%s}' "$2" "$4" |
    curl -sS -o /dev/null -w '%{http_code}' --max-time 10 -X POST -H 'content-type: application/json' \
      --data-binary @- "$3") || true
  if [ "$push_code" = "410" ] && [ "$(sed -n 's/^key=//p' "$1" | head -n 1)" = "$2" ]; then
    rm -rf "${1%/pairing}"
  fi
}

# 方案用量(#98):從 Claude 的用量檔挑出跨過 80／95 且這個週期還沒送過的門檻，逐一送給每支配對裝置。
# 用量檔的格式見 claude-usage/statusline.sh(`status_line_input.rate_limits.{five_hour,seven_day}.{used_percentage,resets_at}`)。
check_usage() {
  _usage_file="$HOME/.purrdr/claude-usage/usage.json"
  [ -f "$_usage_file" ] || return 0
  _json=$(head -c 1048576 "$_usage_file" 2>/dev/null | tr -d '\r\n')
  _now=$(date +%s)
  _sent_file="$state_dir/usage-sent"
  _pending=
  for _period in five_hour seven_day; do
    _block=$(printf '%s' "$_json" | sed -nE 's/.*"'"$_period"'"[[:space:]]*:[[:space:]]*\{([^}]*)\}.*/\1/p')
    # 只取整數部分(比例 85.5 視為 85,門檻是整數所以結果相同;重置時間的小數秒捨去)。
    _used=$(printf '%s' "$_block" | sed -nE 's/.*"used_percentage"[[:space:]]*:[[:space:]]*([0-9]{1,3})([^0-9].*)?$/\1/p')
    _resets=$(printf '%s' "$_block" | sed -nE 's/.*"resets_at"[[:space:]]*:[[:space:]]*([0-9]{1,12})([^0-9].*)?$/\1/p')
    [ -n "$_used" ] && [ -n "$_resets" ] || continue
    # 已過重置時間:那個視窗已經重置，檔案裡的比例過時了。
    [ "$_resets" -gt "$_now" ] || continue
    for _threshold in 80 95; do
      [ "$_used" -ge "$_threshold" ] || continue
      if [ -f "$_sent_file" ] && grep -qxF "$_resets:$_threshold" "$_sent_file"; then continue; fi
      _pending="$_pending $_resets:$_threshold"
    done
  done
  # 已過期週期的紀錄清掉。
  if [ -s "$_sent_file" ]; then
    _kept=$(awk -F: -v now="$_now" '$1 ~ /^[0-9]+$/ && $1 + 0 > now + 0' "$_sent_file")
    if [ "$_kept" != "$(cat "$_sent_file")" ]; then write_state "$_sent_file" "$_kept"; fi
  fi
  [ -n "$_pending" ] || return 0
  _delivered=
  for _file in "$HOME"/.purrdr/push-*/pairing; do
    [ -f "$_file" ] || continue
    _key=$(sed -n 's/^key=//p' "$_file" | head -n 1)
    _endpoint=$(sed -n 's/^endpoint=//p' "$_file" | head -n 1)
    [ -n "$_key" ] && [ -n "$_endpoint" ] || continue
    for _item in $_pending; do
      post_push "$_file" "$_key" "$_endpoint" '"status":"usage","agent":"claude","threshold":'"${_item#*:}"',"resetsAt":'"${_item%:*}"
      case $push_code in 2*) _delivered="$_delivered $_item" ;; esac
      [ "$push_code" != "410" ] || break
    done
  done
  for _item in $_delivered; do
    grep -qxF "$_item" "$_sent_file" 2>/dev/null && continue
    mkdir -p "$state_dir" 2>/dev/null
    printf '%s\n' "$_item" >> "$_sent_file" 2>/dev/null
  done
}

check_usage

# 往下是 blocked／done 的推播;其他狀態(working、idle…)到此為止。
case $status in
  blocked | done) ;;
  *) exit 0 ;;
esac

workspace=$(json_string "${HERDR_PLUGIN_CONTEXT_JSON:-}" workspace_label)
[ -n "$workspace" ] || workspace=$(json_string "${HERDR_PLUGIN_EVENT_JSON:-}" workspace_id)
[ -n "$workspace" ] || exit 0

if [ -n "$pane" ]; then
  extra=',"pane":"'"$pane"'"'
else
  extra=
fi
# 完成才帶工作秒數(算不出來就省略)。
if [ "$status" = done ] && [ -n "$work_seconds" ]; then extra="$extra"',"workSeconds":'"$work_seconds"; fi

session=
tab=
tab_field=
asked=

# 問 herdr 一次:該 pane 的 Agent session(靜音比對用)、Tab label 與 Agent 名稱(推播的 tab 欄位)。
lookup_pane() {
  asked=1
  [ -n "$pane" ] || return 0
  _info=$("${HERDR_BIN_PATH:-herdr}" pane get "$pane" 2>/dev/null)
  session=$(printf '%s' "$_info" | sed -nE 's/.*"agent_session":\{[^}]*"value":"([^"\\]*)".*/\1/p' | head -n 1)
  [ -n "$agent" ] || agent=$(json_string "$_info" agent)
  _tab_id=$(json_string "$_info" tab_id)
  case $_tab_id in
    '' | *[!A-Za-z0-9:_.-]*) return 0 ;;
  esac
  tab=$(json_string "$("${HERDR_BIN_PATH:-herdr}" tab get "$_tab_id" 2>/dev/null)" label)
}

agent=$(json_string "${HERDR_PLUGIN_EVENT_JSON:-}" agent)

# tab 的規則(與主機服務 `tab_name`、notify.ps1 相同):修頭尾空白 → 最多 100 個 UTF-16 單位 → 不切開代理對 → 截斷後再修尾端空白。
# 後台的 tab 最長 100 字元(超過回 400);以 UTF-16 單位計(JavaScript 的 length),不管後台怎麼數字元都不會超過。
# 輸入是 JSON 字串的原樣內容,一個單位是:
#   - `\uXXXX` 與 `\"` 這類跳脫序列(1 個,不被切開,截斷後仍是合法 JSON);
#   - UTF-8 的 1 到 3 位元組字元(1 個);4 位元組字元(emoji 等,UTF-16 是代理對)算 2 個。
# 4 位元組字元的起頭位元組前先放一個 0xFF(UTF-8 不會出現)當第二個單位的記號,取 100 個單位後再拿掉記號;
# 記號後的字元放不下時整個捨棄。跳脫的代理對(`\ud83d\ude00`)是兩個 `\uXXXX`,截斷後若剩落單的高位就拿掉(前面反斜線要成對,才是真的跳脫序列)。
# 單獨的反斜線不算一般字元(否則 `\\` 可以被拆成兩種數法);UTF-8 以位元組處理(LC_ALL=C,字元起頭是非 0x80-0xBF 的位元組),不依賴 herdr 給外掛的 locale。
trim_tab() {
  _low=$(printf '\200')
  _high=$(printf '\277')
  _lead4_first=$(printf '\360')
  _lead4_last=$(printf '\364')
  _marker=$(printf '\377')
  printf '%s' "$1" | LC_ALL=C sed -E -e 's/^[[:space:]]+//' -e 's/[[:space:]]+$//' \
    -e "s/[${_lead4_first}-${_lead4_last}]/${_marker}&/g" \
    -e "s/^((\\\\u[0-9a-fA-F]{4}|\\\\.|${_marker}|[^\\${_low}-${_high}][${_low}-${_high}]*){100}).*\$/\\1/" \
    -e "s/${_marker}//g" \
    -e 's/(^|[^\\])((\\\\)*)\\u[dD][89aAbB][0-9a-fA-F]{2}$/\1\2/' \
    -e 's/[[:space:]]+$//'
}

for file in "$HOME"/.purrdr/push-*/pairing; do
  [ -f "$file" ] || continue
  key=$(sed -n 's/^key=//p' "$file" | head -n 1)
  endpoint=$(sed -n 's/^endpoint=//p' "$file" | head -n 1)
  [ -n "$key" ] && [ -n "$endpoint" ] || continue
  if [ -z "$asked" ]; then
    lookup_pane
    tab=$(trim_tab "$tab")
    agent=$(trim_tab "$agent")
    # 數字(herdr 預設的順序號)不是 Tab label;沒有 Tab label 用 Agent 名稱，都沒有就不帶 tab。
    case $tab in *[!0-9]*) ;; *) tab= ;; esac
    [ -n "$tab" ] || tab=$agent
    if [ -n "$tab" ]; then tab_field=',"tab":"'"$tab"'"'; else tab_field=; fi
  fi
  # 這支裝置靜音了這個聊天(以 Agent 的 session 值識別)→ 跳過。
  if [ -n "$session" ] && grep -qxF "muted=$session" "$file"; then
    continue
  fi
  post_push "$file" "$key" "$endpoint" "$(printf '"workspace":"%s","status":"%s"%s%s' "$workspace" "$status" "$extra" "$tab_field")"
done
