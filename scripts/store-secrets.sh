#!/usr/bin/env bash
# Prompt for each kairos secret once and store it in both places:
# the Mac Keychain and ~/.config/kairos/secrets/ on compute-node.
# Input is hidden; press Enter to skip a secret (an existing value is kept).
set -euo pipefail

HOST="${KAIROS_HOST:-compute-node}"

# server file name | Keychain service | prompt
SECRETS=(
  "apple-id|kairos-apple-id|Apple ID email"
  "icloud-app-password|kairos-icloud|iCloud app-specific password (xxxx-xxxx-xxxx-xxxx)"
)

ssh "$HOST" 'mkdir -p ~/.config/kairos/secrets && chmod 700 ~/.config/kairos/secrets'

for entry in "${SECRETS[@]}"; do
  IFS='|' read -r file service prompt <<<"$entry"
  printf '%s\n> ' "$prompt"
  IFS= read -rs value
  echo
  if [[ -z "$value" ]]; then
    echo "  skipped"
    continue
  fi
  if [[ "$file" == icloud-app-password && ! "$value" =~ ^[a-z]{4}-[a-z]{4}-[a-z]{4}-[a-z]{4}$ ]]; then
    echo "  not an app-specific password (should be 19 characters: xxxx-xxxx-xxxx-xxxx, lowercase letters); skipped" >&2
    continue
  fi
  if [[ "$file" == ics-* && ! "$value" =~ ^https://calendar\.google\.com/calendar/ical/.+\.ics$ ]]; then
    echo "  doesn't look like a Google iCal address (https://calendar.google.com/calendar/ical/…/basic.ics); skipped" >&2
    continue
  fi
  security add-generic-password -U -s "$service" -a "$USER" -w "$value"
  printf '%s' "$value" | ssh "$HOST" "umask 077; cat > ~/.config/kairos/secrets/$file"
  echo "  stored (Keychain: $service, $HOST: secrets/$file)"
done

echo
echo "Restarting kairosd so it picks up the new secrets…"
ssh "$HOST" 'systemctl --user restart kairosd && sleep 1 && systemctl --user is-active kairosd'
