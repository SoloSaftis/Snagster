#!/bin/sh
# Runs as a normal user inside dbus-run-session: a headless GNOME Shell (Wayland), PipeWire and
# the GNOME portal, then Snagster asks for a screenshot and records, while gnome-helper.py takes
# pictures of the screen (to see GNOME's dialogs) and presses the keys given in $KEYS_SHOT and
# $KEYS_CAST. Results go to $OUT.
set -u
OUT="${OUT:-$HOME/out}"
SNAG="${SNAG:-$HOME/Snagster/Snagster}"
HERE="$(dirname "$(readlink -f "$0")")"
H="python3 $HERE/gnome-helper.py"
mkdir -p "$OUT"
export XDG_RUNTIME_DIR="/tmp/xdg-$(id -u)"
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_DESKTOP=gnome
unset DISPLAY WAYLAND_DISPLAY

pipewire > "$OUT/pipewire.log" 2>&1 &
sleep 1
wireplumber > "$OUT/wireplumber.log" 2>&1 &
gnome-shell --headless --wayland --no-x11 --virtual-monitor 1920x1080 > "$OUT/shell.log" 2>&1 &
for _ in $(seq 60); do [ -S "$XDG_RUNTIME_DIR/wayland-0" ] && break; sleep 0.5; done
export WAYLAND_DISPLAY=wayland-0
dbus-update-activation-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE XDG_SESSION_DESKTOP XDG_RUNTIME_DIR
sleep 6
$H shot:"$OUT/00-desktop.png"

echo "== Screenshot through the portal" | tee -a "$OUT/result.txt"
"$SNAG" capture all -o "$OUT/capture.png" > "$OUT/capture.txt" 2>&1 &
P=$!
sleep 4
$H shot:"$OUT/01-screenshot-dialog.png"
if kill -0 $P 2>/dev/null; then $H ${KEYS_SHOT:-} sleep:2 shot:"$OUT/02-after-keys.png"; fi
for _ in $(seq 20); do kill -0 $P 2>/dev/null || break; sleep 1; done
kill $P 2>/dev/null && echo "capture: still waiting after 20 s" | tee -a "$OUT/result.txt"
wait $P 2>/dev/null; echo "capture exit $?" | tee -a "$OUT/result.txt"
[ -s "$OUT/capture.png" ] && echo "capture.png: $(wc -c < "$OUT/capture.png") bytes" | tee -a "$OUT/result.txt"

echo "== Recording through the portal" | tee -a "$OUT/result.txt"
"$SNAG" --video-test quick > "$OUT/video.txt" 2>&1 &
V=$!
i=10
while kill -0 $V 2>/dev/null && [ $i -lt 40 ]; do
  sleep 4
  $H shot:"$OUT/$i-cast.png" ${KEYS_CAST:-}
  i=$((i + 1))
done
kill $V 2>/dev/null && echo "video test: still running, stopped" | tee -a "$OUT/result.txt"
cat "$OUT/video.txt" >> "$OUT/result.txt"
