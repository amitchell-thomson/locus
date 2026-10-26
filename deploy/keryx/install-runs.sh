#!/usr/bin/env bash
# Install the scheduled Claude runs (com.keryx.runs): `keryx run auto` at the
# morning / evening times (keep them equal to [runs] in config.toml) (default 08:00 and 18:00). Needs keryxd
# installed (deploy/keryx/install-daemon.sh) and Claude Code logged in.
# Times: KERYX_MORNING=07:30 KERYX_EVENING=18:30 deploy/keryx/install-runs.sh
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
morning="${KERYX_MORNING:-08:00}"
evening="${KERYX_EVENING:-18:00}"
num() { echo $((10#$1)); }
mh=$(num "${morning%%:*}"); mm=$(num "${morning##*:}")
eh=$(num "${evening%%:*}"); em=$(num "${evening##*:}")
command -v claude >/dev/null || { echo "claude isn't on PATH: install Claude Code first" >&2; exit 1; }
"$HOME/.cargo/bin/keryx" run morning --dry-run >/dev/null
dst="$HOME/Library/LaunchAgents/com.keryx.runs.plist"
sed -e "s|__HOME__|$HOME|g" -e "s|__MORNING_H__|$mh|" -e "s|__MORNING_M__|$mm|" \
    -e "s|__EVENING_H__|$eh|" -e "s|__EVENING_M__|$em|" deploy/keryx/launchd/com.keryx.runs.plist > "$dst"
plutil -lint "$dst" >/dev/null
launchctl bootout "gui/$(id -u)/com.keryx.runs" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$dst"
echo "runs scheduled at $morning and $evening; log: ~/Library/Logs/keryx-runs.log"
echo "Try one now: keryx run morning --dry-run, then keryx run morning"
