#!/usr/bin/env bash
# locus-outbox.sh — push files from a local drop folder to the Locus server's
# vault/incoming over SSH, deleting each local file ONLY after a confirmed transfer.
#
# One-way (outbox) sync: this script never pulls. Nothing the server does to
# vault/incoming/ (the watcher consuming/deleting files) can propagate back to this Mac.
#
# Driven on an interval by a launchd agent (com.locus.outbox.plist). Safe to run by hand.
set -euo pipefail

CONF="${LOCUS_OUTBOX_CONF:-$HOME/.config/locus-outbox/outbox.conf}"
if [[ ! -f "$CONF" ]]; then
  echo "locus-outbox: config not found at $CONF (copy outbox.conf.example and edit)" >&2
  exit 1
fi
# shellcheck source=/dev/null
source "$CONF"

# Required settings (fail loudly if the conf is incomplete).
: "${REMOTE:?set REMOTE in $CONF}"
: "${REMOTE_DIR:?set REMOTE_DIR in $CONF}"
: "${DROP_DIR:?set DROP_DIR in $CONF}"

# Defaults (overridable in the conf).
RSYNC="${RSYNC:-rsync}"
SSH_OPTS="${SSH_OPTS:-ssh -o ConnectTimeout=10 -o BatchMode=yes}"

mkdir -p "$DROP_DIR"

# Nothing meaningful to send? (ignore macOS noise like .DS_Store) -> quiet exit.
if [[ -z "$(find "$DROP_DIR" -mindepth 1 -type f -not -name '.DS_Store' -print -quit)" ]]; then
  exit 0
fi

# Prevent overlapping runs: a slow transfer must not collide with the next interval tick.
# mkdir is atomic, so it doubles as a lock that we remove on exit.
LOCK="${TMPDIR:-/tmp}/locus-outbox.lock"
if ! mkdir "$LOCK" 2>/dev/null; then
  echo "locus-outbox: a previous run is still active, skipping this tick" >&2
  exit 0
fi
trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT

ts() { date '+%Y-%m-%d %H:%M:%S'; }
echo "[$(ts)] flushing $DROP_DIR -> $REMOTE:$REMOTE_DIR"

# -r recurse, -l symlinks, -t times, -z compress (good over WAN).
# --remove-source-files deletes a source file only after it transfers successfully;
#   if we're offline the transfer fails, the file stays, and the next tick retries it.
# We deliberately do NOT use -p/-o/-g (perms/owner/group): they cause noise mapping a Mac
#   user onto the Linux server and add nothing for ingest.
"$RSYNC" -rltz --remove-source-files \
  --exclude='.DS_Store' --exclude='._*' \
  --exclude='.Spotlight-V100' --exclude='.Trashes' --exclude='.fseventsd' \
  -e "$SSH_OPTS" \
  "$DROP_DIR"/ "$REMOTE:$REMOTE_DIR/"

# Clean up the drop folder. rsync left the macOS noise files behind (we exclude
# .DS_Store/._*/etc. from transfer), and a subfolder still holding one of those is
# not -empty, so it would survive the sweep below. Purge that junk first, then the
# now-truly-empty subdirectories.
find "$DROP_DIR" -mindepth 1 -type f \
  \( -name '.DS_Store' -o -name '._*' \) -delete
find "$DROP_DIR" -mindepth 1 -type d \
  \( -name '.Spotlight-V100' -o -name '.Trashes' -o -name '.fseventsd' \) \
  -exec rm -rf {} +
# Tidy now-empty subdirectories left behind by dropped folders-of-files — but only at
# depth 2+: the FIRST-level folders are the owner's permanent category taxonomy
# (papers/, coursework/, projects/, career/, notes/), which the server derives
# `documents.category` from, and they must survive being emptied by a flush. Never
# remove the drop folder itself either.
find "$DROP_DIR" -mindepth 2 -type d -empty -delete

echo "[$(ts)] done"
