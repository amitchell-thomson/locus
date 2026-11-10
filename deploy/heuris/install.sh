#!/usr/bin/env bash
# Install the laptop's heuris push (com.locus.heuris-push): `locus heuris-push` every 15 minutes.
# Run once per laptop, and again after editing com.locus.heuris-push.plist.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
uv="$(command -v uv)" || { echo "uv isn't on PATH" >&2; exit 1; }
dst="$HOME/Library/LaunchAgents/com.locus.heuris-push.plist"
sed -e "s|__HOME__|$HOME|g" -e "s|__REPO__|$repo|g" -e "s|__UV__|$uv|g" \
    "$repo/deploy/heuris/com.locus.heuris-push.plist" > "$dst"
plutil -lint "$dst" >/dev/null
launchctl bootout "gui/$(id -u)/com.locus.heuris-push" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$dst"
echo "heuris push every 15 min; log: ~/Library/Logs/locus-heuris-push.log"
