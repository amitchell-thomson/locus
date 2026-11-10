#!/usr/bin/env bash
# Install the laptop's transcript push (com.locus.transcripts): `locus transcripts-push` every 30 minutes.
# Run once per laptop, and again after editing com.locus.transcripts.plist.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
uv="$(command -v uv)" || { echo "uv isn't on PATH" >&2; exit 1; }
dst="$HOME/Library/LaunchAgents/com.locus.transcripts.plist"
sed -e "s|__HOME__|$HOME|g" -e "s|__REPO__|$repo|g" -e "s|__UV__|$uv|g" \
    "$repo/deploy/transcripts/com.locus.transcripts.plist" > "$dst"
plutil -lint "$dst" >/dev/null
launchctl bootout "gui/$(id -u)/com.locus.transcripts" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$dst"
echo "transcript push every 30 min; log: ~/Library/Logs/locus-transcripts.log"
