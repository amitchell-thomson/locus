#!/usr/bin/env bash
# Install the laptop's folder sync (com.locus.files): `locus files sync` every 30 minutes.
# Run once per laptop, and again after editing com.locus.files.plist.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
uv="$(command -v uv)" || { echo "uv isn't on PATH" >&2; exit 1; }
dst="$HOME/Library/LaunchAgents/com.locus.files.plist"
sed -e "s|__HOME__|$HOME|g" -e "s|__REPO__|$repo|g" -e "s|__UV__|$uv|g" \
    "$repo/deploy/files/com.locus.files.plist" > "$dst"
plutil -lint "$dst" >/dev/null
launchctl bootout "gui/$(id -u)/com.locus.files" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$dst"
echo "folder sync every 30 min; log: ~/Library/Logs/locus-files.log; questions: locus files questions"
