#!/bin/sh
# Snagster through real desktop portals, with no screen: a headless sway (wlroots) desktop,
# PipeWire, and xdg-desktop-portal with its wlr backend set to share the only screen without
# asking. Then a still capture (Screenshot portal) and the quick recording test (ScreenCast
# portal over PipeWire). Used by CI's Linux job; run it on Ubuntu with:
#
#   sudo apt-get install -y sway xdg-desktop-portal xdg-desktop-portal-wlr pipewire wireplumber dbus
#   dbus-run-session -- sh tests/portal-test.sh target/release/snagster
#
# GNOME's and KDE's own portal dialogs still need a person to click "Share"; this checks
# everything else on Snagster's side (D-Bus requests, the PipeWire stream, the pictures).
set -eu
SNAG="$(readlink -f "$1")"
work="$(mktemp -d)"
export XDG_RUNTIME_DIR="$work/run" XDG_CONFIG_HOME="$work/config" HOME="$work/home"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME/xdg-desktop-portal" "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr" "$HOME"
chmod 700 "$XDG_RUNTIME_DIR"
unset DISPLAY WAYLAND_DISPLAY
export XDG_CURRENT_DESKTOP=sway XDG_SESSION_TYPE=wayland

printf 'output HEADLESS-1 resolution 1920x1080\nxwayland disable\n' > "$work/sway.conf"
printf '[preferred]\ndefault=wlr\n' > "$XDG_CONFIG_HOME/xdg-desktop-portal/portals.conf"
cp "$XDG_CONFIG_HOME/xdg-desktop-portal/portals.conf" "$XDG_CONFIG_HOME/xdg-desktop-portal/sway-portals.conf"
printf '[screencast]\nchooser_type=none\noutput_name=HEADLESS-1\n' > "$XDG_CONFIG_HOME/xdg-desktop-portal-wlr/config"

cleanup() { kill $(jobs -p) 2>/dev/null || true; }
trap cleanup EXIT

WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 sway -c "$work/sway.conf" > "$work/sway.log" 2>&1 &
for _ in $(seq 50); do
  sock="$(ls "$XDG_RUNTIME_DIR" | grep -E '^wayland-[0-9]+$' | head -1 || true)"
  [ -n "$sock" ] && break
  sleep 0.2
done
[ -n "$sock" ] || { echo "portal test: sway didn't start"; cat "$work/sway.log"; exit 1; }
export WAYLAND_DISPLAY="$sock"
dbus-update-activation-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_SESSION_TYPE 2>/dev/null || true

pipewire > "$work/pipewire.log" 2>&1 &
sleep 1
wireplumber > "$work/wireplumber.log" 2>&1 &
/usr/libexec/xdg-desktop-portal-wlr -l DEBUG > "$work/portal-wlr.log" 2>&1 &
sleep 1
/usr/libexec/xdg-desktop-portal -r > "$work/portal.log" 2>&1 &
sleep 2

status=0
echo "== Still capture through the Screenshot portal"
if "$SNAG" capture all -o "$work/shot.png" && [ -s "$work/shot.png" ]; then
  echo "ok   capture all: $(wc -c < "$work/shot.png") bytes"
else
  echo "FAIL capture all"; status=1
fi
echo "== Recording through the ScreenCast portal"
"$SNAG" --video-test quick | tee "$work/video.txt" || true
grep -q "^video test: all passed" "$work/video.txt" || status=1
grep -q "screen share (portal)" "$work/video.txt" || echo "note: the recording didn't report the portal as its capture"

if [ "$status" -ne 0 ]; then
  for f in sway portal portal-wlr pipewire wireplumber; do echo "--- $f.log"; tail -20 "$work/$f.log"; done
fi
exit "$status"
