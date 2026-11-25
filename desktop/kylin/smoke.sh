#!/usr/bin/env bash
set -euo pipefail

window=$(xdotool search --sync --onlyvisible --class 'com.guanhai.bafen' | head -1)
xprop -id "$window" WM_CLASS _NET_WM_ICON > /tmp/guanhai-window.properties
grep -F 'WM_CLASS(STRING) = "com.guanhai.bafen", "com.guanhai.bafen"' /tmp/guanhai-window.properties
grep -F '_NET_WM_ICON(CARDINAL)' /tmp/guanhai-window.properties
timeout 10s /usr/bin/guanhai-bafen
test "$(xdotool search --onlyvisible --class 'com.guanhai.bafen' | wc -l)" = 1

# The initial input picker is only reachable once the JavaScript UI has loaded.
sleep 4
xdotool windowfocus --sync "$window" key Tab Tab Tab Tab Tab Tab Tab Return
timeout 15s xdotool search --sync --onlyvisible --name '选择输入文件或文件夹'
xdotool key Escape
# Request a normal window close and verify the application can be restarted.
xdotool windowfocus --sync "$window" key alt+F4
timeout 15s tail --pid="$APP_PID" -f /dev/null
/usr/bin/guanhai-bafen &
restarted=$!
trap 'kill "$restarted" 2>/dev/null || true' EXIT
timeout 20s xdotool search --sync --onlyvisible --class 'com.guanhai.bafen'
