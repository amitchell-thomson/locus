#!/usr/bin/env bash
# Copy kairos templates into the chezmoi source dir and set up .chezmoiignore.
# Review with `chezmoi diff`, then `chezmoi apply`.
set -euo pipefail
SRC="$(chezmoi source-path)"
HERE="$(cd "$(dirname "$0")" && pwd)/chezmoi"
mkdir -p "$SRC/dot_config/kairos" "$SRC/dot_config/vdirsyncer" "$SRC/dot_config/systemd/user" "$SRC/Library/LaunchAgents"
cp "$HERE/dot_config/kairos/config.toml.tmpl" "$SRC/dot_config/kairos/"
cp "$HERE/dot_config/kairos/constraints.toml" "$SRC/dot_config/kairos/"
cp "$HERE/dot_config/vdirsyncer/config.tmpl" "$SRC/dot_config/vdirsyncer/"
cp "$HERE/dot_config/systemd/user/kairosd.service" "$SRC/dot_config/systemd/user/"
cp "$HERE/Library/LaunchAgents/com.kairos.sync.plist.tmpl" "$SRC/Library/LaunchAgents/"
IGN="$SRC/.chezmoiignore"
if ! grep -q 'kairos' "$IGN" 2>/dev/null; then
  cat >> "$IGN" <<'IG'
# kairos
{{ if ne .chezmoi.os "darwin" }}
Library/LaunchAgents/com.kairos.sync.plist
{{ end }}
{{ if ne .chezmoi.os "linux" }}
.config/systemd/user/kairosd.service
{{ end }}
IG
fi
echo "Installed into $SRC. Now: chezmoi diff && chezmoi apply"
