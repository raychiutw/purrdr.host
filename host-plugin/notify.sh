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

status=$(printf '%s' "${HERDR_PLUGIN_EVENT_JSON:-}" | sed -nE 's/.*"agent_status":"([a-z_]*)".*/\1/p')
case $status in
  blocked | done) ;;
  *) exit 0 ;;
esac

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

# 取出 JSON 字串欄位的值，維持 JSON 的跳脫(原樣放回請求的 JSON 裡)。
json_string() {
  printf '%s' "$1" | sed -nE 's/.*"'"$2"'":"(([^"\\]|\\.)*)".*/\1/p' | head -n 1
}

pane=$(json_string "${HERDR_PLUGIN_EVENT_JSON:-}" pane_id)
case $pane in
  '' | *[!A-Za-z0-9:_.-]*) pane= ;;
esac
workspace=$(json_string "${HERDR_PLUGIN_CONTEXT_JSON:-}" workspace_label)
[ -n "$workspace" ] || workspace=$(json_string "${HERDR_PLUGIN_EVENT_JSON:-}" workspace_id)
[ -n "$workspace" ] || exit 0

if [ -n "$pane" ]; then
  extra=',"pane":"'"$pane"'"'
else
  extra=
fi

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
  code=$(printf '{"key":"%s","workspace":"%s","status":"%s"%s%s}' "$key" "$workspace" "$status" "$extra" "$tab_field" |
    curl -sS -o /dev/null -w '%{http_code}' --max-time 10 -X POST -H 'content-type: application/json' \
      --data-binary @- "$endpoint") || true
  # 送出期間重新配對了(密鑰換了)就不是被撤銷的那一組，不刪。
  if [ "$code" = "410" ] && [ "$(sed -n 's/^key=//p' "$file" | head -n 1)" = "$key" ]; then
    rm -rf "${file%/pairing}"
  fi
done
