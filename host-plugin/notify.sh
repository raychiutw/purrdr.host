#!/bin/sh
# Purrdr 主機外掛(macOS／Linux):herdr 在 pane.agent_status_changed 時以新程序執行這個腳本。
#
# Agent 進入 blocked(「需要你」)或 done(「完成」)時，對 ~/.purrdr/push-*/pairing 的每一支配對裝置呼叫 Purrdr 後台的 /v1/push;
# 是否真的通知由後台依裝置偏好決定。
# 請求只帶 workspace 名稱、狀態與 pane id(推播內容由後台組),不帶終端機標題或任何對話內容。
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
asked=
for file in "$HOME"/.purrdr/push-*/pairing; do
  [ -f "$file" ] || continue
  key=$(sed -n 's/^key=//p' "$file" | head -n 1)
  endpoint=$(sed -n 's/^endpoint=//p' "$file" | head -n 1)
  [ -n "$key" ] && [ -n "$endpoint" ] || continue
  # 這支裝置靜音了這個聊天(以 Agent 的 session 值識別)→ 跳過。有靜音時才問 herdr 一次。
  if grep -q '^muted=' "$file"; then
    if [ -z "$asked" ] && [ -n "$pane" ]; then
      asked=1
      session=$("${HERDR_BIN_PATH:-herdr}" pane get "$pane" 2>/dev/null |
        sed -nE 's/.*"agent_session":\{[^}]*"value":"([^"\\]*)".*/\1/p' | head -n 1)
    fi
    if [ -n "$session" ] && grep -qxF "muted=$session" "$file"; then
      continue
    fi
  fi
  code=$(printf '{"key":"%s","workspace":"%s","status":"%s"%s}' "$key" "$workspace" "$status" "$extra" |
    curl -sS -o /dev/null -w '%{http_code}' --max-time 10 -X POST -H 'content-type: application/json' \
      --data-binary @- "$endpoint") || true
  # 送出期間重新配對了(密鑰換了)就不是被撤銷的那一組，不刪。
  if [ "$code" = "410" ] && [ "$(sed -n 's/^key=//p' "$file" | head -n 1)" = "$key" ]; then
    rm -rf "${file%/pairing}"
  fi
done
