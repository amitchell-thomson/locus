#!/usr/bin/env bash
# Install keryxd and its launchd agent (com.keryx.daemon). Re-run after updates.
set -euo pipefail
# Run from anywhere: paths are the locus repository's (keryx moved in on 26 Oct 2026).
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
# A different crate called `keryx` from crates.io would block this install.
if cargo install --list | grep -q '^keryx v[0-9.]*:$'; then
  echo "an unrelated 'keryx' from crates.io is installed; remove it first: cargo uninstall keryx" >&2
  exit 1
fi
cargo install --locked --path keryx/crates/keryxd
cargo install --locked --path keryx/crates/keryx-cli
dst="$HOME/Library/LaunchAgents/com.keryx.daemon.plist"
sed "s|__HOME__|$HOME|g" deploy/keryx/launchd/com.keryx.daemon.plist > "$dst"
launchctl bootout "gui/$(id -u)/com.keryx.daemon" 2>/dev/null || true
# bootout returns before the old keryxd has gone; loading again too soon fails with
# "Bootstrap failed: 5" and leaves no keryxd at all (26 Oct). Wait, then retry once.
for _ in $(seq 1 20); do
    launchctl print "gui/$(id -u)/com.keryx.daemon" >/dev/null 2>&1 || break
    sleep 0.5
done
launchctl bootstrap "gui/$(id -u)" "$dst" || { sleep 2; launchctl bootstrap "gui/$(id -u)" "$dst"; }
echo "keryxd running; log: ~/Library/Logs/keryxd.log"
echo "If this is the first install (or doctor reports no access), grant Full Disk Access to ~/.cargo/bin/keryxd (keryx/docs/setup.md §3)."
