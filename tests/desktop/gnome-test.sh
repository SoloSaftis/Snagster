#!/bin/sh
# Run as root on a fresh Ubuntu 24.04 machine (for example through AWS SSM):
#   sh gnome-test.sh <git ref> <Snagster release tag> <upload URL>
# Installs a headless GNOME, takes Snagster from the release, runs gnome-session.sh as a normal
# user and uploads the results (a .tgz of pictures and logs) with an HTTP PUT to the given URL.
set -eu
REF="$1"; TAG="$2"; PUT="$3"
export DEBIAN_FRONTEND=noninteractive
if ! command -v gnome-shell >/dev/null; then
  apt-get update -q
  apt-get install -y -q --no-install-recommends gnome-shell mutter xdg-desktop-portal xdg-desktop-portal-gnome \
    pipewire wireplumber dbus-user-session gstreamer1.0-pipewire gstreamer1.0-tools gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good python3-gi gir1.2-glib-2.0 libgl1-mesa-dri libegl1 curl ca-certificates \
    libxkbcommon0 libwayland-client0 libgbm1 > /root/apt.log 2>&1
fi
id snag >/dev/null 2>&1 || useradd -m snag
H=/home/snag
rm -rf "$H/out" "$H/t"; mkdir -p "$H/t"
for f in gnome-helper.py gnome-session.sh; do
  curl -fsSL -o "$H/t/$f" "https://raw.githubusercontent.com/SoloSaftis/Snagster/$REF/tests/desktop/$f"
done
if [ ! -x "$H/Snagster/Snagster" ] || [ "$(cat "$H/tag" 2>/dev/null)" != "$TAG" ]; then
  curl -fsSL -o "$H/s.tgz" "https://github.com/SoloSaftis/Snagster/releases/download/$TAG/Snagster-linux-x86_64-portable.tar.gz"
  rm -rf "$H/Snagster"; tar -C "$H" -xzf "$H/s.tgz"; echo "$TAG" > "$H/tag"
fi
chown -R snag: "$H"
su - snag -c "KEYS_SHOT='${KEYS_SHOT:-}' KEYS_CAST='${KEYS_CAST:-}' timeout 300 dbus-run-session -- sh $H/t/gnome-session.sh" > "$H/session.txt" 2>&1 || true
cp "$H/session.txt" "$H/out/" 2>/dev/null || true
tar -C "$H" -czf /root/results.tgz out
curl -fsS -X PUT -T /root/results.tgz "$PUT" && echo "uploaded $(wc -c < /root/results.tgz) bytes"
tail -30 "$H/out/result.txt" 2>/dev/null
