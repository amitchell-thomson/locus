#!/usr/bin/env bash
# Install keryxd and its launchd agent (com.keryx.daemon). Re-run after updates.
set -euo pipefail
cd "$(dirname "$0")/.."
# A different crate called `keryx` from crates.io would block this install.
if cargo install --list | grep -q '^keryx v[0-9.]*:$'; then
  echo "an unrelated 'keryx' from crates.io is installed; remove it first: cargo uninstall keryx" >&2
  exit 1
fi
cargo install --locked --path crates/keryxd
cargo install --locked --path crates/keryx-cli
dst="$HOME/Library/LaunchAgents/com.keryx.daemon.plist"
sed "s|__HOME__|$HOME|g" deploy/launchd/com.keryx.daemon.plist > "$dst"
launchctl bootout "gui/$(id -u)/com.keryx.daemon" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$dst"
echo "keryxd running; log: ~/Library/Logs/keryxd.log"
echo "If this is the first install (or doctor reports no access), grant Full Disk Access to ~/.cargo/bin/keryxd (docs/setup.md §3)."
