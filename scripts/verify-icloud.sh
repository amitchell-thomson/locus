#!/usr/bin/env bash
# Phase 1 verification, run on compute-node once the iCloud secrets exist.
#   scripts/verify-icloud.sh                    discover + sync, Family check, X-KAIROS round-trip
#   scripts/verify-icloud.sh --after-phone-edit re-check X-KAIROS after editing the test event on the iPhone
# Writes only to ZZ-kairos-test (kairosd must be running with live = false).
set -euo pipefail
VDIR="$HOME/.local/share/kairos/vdir"
STATE="$HOME/.local/share/kairos/roundtrip-uid"
API="${KAIROS_API:-http://100.117.10.28:7420}"
TOKEN="$(cat "$HOME/.config/kairos/secrets/api-token")"
ok() { printf '\033[32m✓\033[0m %s\n' "$*"; }
bad() { printf '\033[31m✗\033[0m %s\n' "$*"; }

fresh_fetch() {
  # Download a fresh copy straight from iCloud into a temp dir (read-only remote).
  local tmp; tmp="$(mktemp -d)"
  cat > "$tmp/config" <<CFG
[general]
status_path = "$tmp/status/"
[pair check]
a = "remote"
b = "local"
collections = ["from a"]
[storage remote]
type = "caldav"
url = "https://caldav.icloud.com/"
item_types = ["VEVENT"]
username.fetch = ["command", "cat", "$HOME/.config/kairos/secrets/apple-id"]
password.fetch = ["command", "cat", "$HOME/.config/kairos/secrets/icloud-app-password"]
read_only = true
[storage local]
type = "filesystem"
path = "$tmp/vdir/"
fileext = ".ics"
CFG
  { yes || true; } | vdirsyncer -c "$tmp/config" discover check >/dev/null 2>&1 || true
  vdirsyncer -c "$tmp/config" sync >/dev/null 2>&1 || true
  echo "$tmp"
}

if [[ "${1:-}" == "--after-phone-edit" ]]; then
  uid="$(cat "$STATE")"
  tmp="$(fresh_fetch)"
  f="$(grep -rl "UID:$uid" "$tmp/vdir" | head -1 || true)"
  [[ -n "$f" ]] || { bad "test event $uid not found on iCloud"; exit 1; }
  grep -m1 '^SUMMARY' "$f"
  if grep -q '^X-KAIROS-' "$f"; then ok "X-KAIROS-* survived the iPhone edit"; else bad "iPhone edit stripped X-KAIROS-* (sidecar remains authoritative)"; fi
  echo "Record the result in CLAUDE.md → Phase 1 findings."
  exit 0
fi

echo "== discover + sync"
{ yes || true; } | vdirsyncer discover icloud
vdirsyncer sync
vdirsyncer metasync
echo "== collections"
found_family=0
for d in "$VDIR"/*/; do
  name="$(cat "$d/displayname" 2>/dev/null || echo '?')"
  printf '  %-40s %s (%s items)\n' "$(basename "$d")" "$name" "$(ls "$d" | grep -c '\.ics$' || true)"
  [[ "$name" == "Family" ]] && found_family=1
done
if [[ $found_family == 1 ]]; then ok "Family calendar is discoverable over CalDAV"; else bad "Family calendar NOT found over CalDAV — stop and report options (see SPEC §9 Phase 1)"; fi
kairos doctor || true

echo "== X-KAIROS round-trip via ZZ-kairos-test"
start="$(date -u -d 'tomorrow 12:00' +%Y-%m-%dT%H:%M:00Z)"
resp="$(curl -sf -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d "{\"title\":\"kairos round-trip test\",\"start\":\"$start\",\"duration_min\":15}" "$API/api/events")"
id="$(python3 -c 'import json,sys;print(json.loads(sys.argv[1])["changeset_id"])' "$resp")"
uid="$(curl -sf -H "Authorization: Bearer $TOKEN" "$API/api/changesets/$id" | python3 -c 'import json,sys;print(json.load(sys.stdin)["ops"][0]["uid"])')"
echo "$uid" > "$STATE"
sleep 8
tmp="$(fresh_fetch)"
f="$(grep -rl "UID:$uid" "$tmp/vdir" | head -1 || true)"
if [[ -z "$f" ]]; then bad "event didn't reach iCloud (check: journalctl --user -u kairosd)"; exit 1; fi
ok "event reached iCloud ($(basename "$(dirname "$f")"))"
if grep -q '^X-KAIROS-CREATED-BY' "$f"; then ok "X-KAIROS-* survived the iCloud round-trip"; else bad "iCloud stripped X-KAIROS-* (sidecar remains authoritative)"; fi
echo
echo "Next: on the iPhone, open 'kairos round-trip test' (tomorrow 12:00, ZZ-kairos-test), change its title, wait a minute, then run:"
echo "  scripts/verify-icloud.sh --after-phone-edit"
echo "Undo the test event afterwards: kairos undo $id"
