#!/bin/sh
# Locus capture agent v5 — renders every changed document with xochitl's own renderer
# and pushes it to the server over the tailnet.
#
# v5 fixes the WRONG-DOCUMENT bug (confirmed 2026-07-28). v4 fetched every render to a
# single shared $LOC/out.pdf and gated the push on file SIZE alone, never on wget's exit
# status. BusyBox `wget -O FILE` does NOT create or truncate FILE when the connection is
# refused (verified on-device: a 204800-byte file survived a failed fetch byte-identical),
# so once xochitl's web interface died mid-run, every subsequent document re-pushed the
# LAST SUCCESSFULLY RENDERED PDF under its own uuid. The 09:26 run logged
# "checked=17 pushed=17 failed=0" while pushing 14 byte-identical copies of one notebook.
#
# The invariant now enforced before anything is pushed or recorded:
#   1. the temp file is removed before each fetch (no previous render can survive)
#   2. wget's exit status is checked (the check v4 lacked)
#   3. the file exists, is >= MIN_PDF bytes, and starts with the %PDF magic
#   4. its md5 differs from the render pushed immediately before it (defence in depth:
#      this is the exact signature of the v4 bug, so it is worth alarming on directly)
# and the manifest entry is written ONLY after the push itself succeeds, so a failed push
# no longer marks a document as delivered (v4 recorded it unconditionally -> silent loss).
#
# A render failure now also probes the web interface and ABORTS the run if it is down,
# instead of grinding through every remaining document. v4's 18-consecutive-failure runs
# were pure noise.
#
# Change key = the doc's lastModified value (bumps on EDITS, not on mere opens), so
# browsing/reading does not trigger re-pushes. Falls back to md5(.metadata).
#
# Reversible: v4 is kept at render-push.v4.sh by the install step; restore by copying back.

XDIR=/home/root/.local/share/remarkable/xochitl
LOC=/home/root/locus
MAN=$LOC/manifest
LG=$LOC/agent.log
LOCK=$LOC/agent.lock
OUT=$LOC/out.pdf
PKT=$LOC/pkt
SRV=100.117.10.28
PORT=9010
TS=/home/root/tailscale_1.98.9_arm64/tailscale
SOCK=/home/root/.tailscale/tailscaled.sock
WEB=http://10.11.99.1
MIN_PDF=1000
PUSH_TIMEOUT=90
# BusyBox wget -T is an INACTIVITY timeout, and xochitl generates the entire PDF before
# sending a single byte -- so a long render is indistinguishable from a dead connection.
# Small notebooks return in 0-7s, but a 211-page one (3.8MB) took minutes of pure silence
# on the Paper Pro. Deliberately generous: a genuinely dead interface is caught by the
# web_up() probes, NOT by this timeout, so the only thing a tight value can achieve is
# misreading a slow render as a failure and dropping that document.
RENDER_TIMEOUT=900

mkdir -p "$LOC"; touch "$MAN"

if [ -f "$LOCK" ] && kill -0 "$(cat "$LOCK" 2>/dev/null)" 2>/dev/null; then exit 0; fi
echo $$ > "$LOCK"
# v4 leaked a stale out.pdf whenever a run was killed mid-loop (the tablet auto-suspends);
# that leftover is what the next run then re-pushed. Always clean up, on every exit path.
trap 'rm -f "$LOCK" "$PKT" "$OUT"' EXIT INT TERM

log() { echo "$*" >> "$LG"; }

log "=== run $(date) ==="

web_up() { wget -q -T 8 -O /dev/null "$WEB/documents/" 2>/dev/null; }

# Fetch one document. Returns 0 only if a genuinely new, valid PDF is now at $OUT.
# Sets $sz. Every early return leaves no usable file behind.
render_one() {
  _uuid=$1
  rm -f "$OUT"                                   # (1) no previous render can survive
  wget -q -T $RENDER_TIMEOUT -O "$OUT" "$WEB/download/$_uuid/pdf" 2>/dev/null
  _rc=$?
  if [ $_rc -ne 0 ]; then                        # (2) the check v4 lacked
    log "render-fail $_uuid wget-rc=$_rc"
    rm -f "$OUT"
    return 1
  fi
  if [ ! -f "$OUT" ]; then
    log "render-fail $_uuid no-output-file"
    return 1
  fi
  sz=$(wc -c < "$OUT" 2>/dev/null || echo 0)
  if [ "${sz:-0}" -lt $MIN_PDF ]; then           # (3) size
    log "render-fail $_uuid short sz=$sz"
    rm -f "$OUT"
    return 1
  fi
  if [ "$(dd if="$OUT" bs=1 count=4 2>/dev/null)" != "%PDF" ]; then
    log "render-fail $_uuid not-a-pdf sz=$sz"
    rm -f "$OUT"
    return 1
  fi
  return 0
}

# Push $OUT with its header. Returns the transport's exit status, not a guess.
push_one() {
  _uuid=$1; _key=$2; _sz=$3
  { printf 'LOCUSDOC %s %s %s\n' "$_uuid" "$_key" "$_sz"; cat "$OUT"; } > "$PKT"
  if [ "$HAVE_TIMEOUT" = 1 ]; then
    timeout $PUSH_TIMEOUT $TS --socket=$SOCK nc $SRV $PORT < "$PKT"
    _prc=$?
  else
    # No busybox `timeout`: background + poll, then reap for the real status. If we had to
    # kill it, it did not finish -> failure, so the doc is retried on the next run.
    $TS --socket=$SOCK nc $SRV $PORT < "$PKT" & _p=$!
    _n=0
    while kill -0 $_p 2>/dev/null && [ $_n -lt $PUSH_TIMEOUT ]; do sleep 1; _n=$((_n+1)); done
    if kill -0 $_p 2>/dev/null; then
      kill $_p 2>/dev/null; wait $_p 2>/dev/null; _prc=124
    else
      wait $_p; _prc=$?
    fi
  fi
  rm -f "$PKT"
  return $_prc
}

if command -v timeout >/dev/null 2>&1; then HAVE_TIMEOUT=1; else HAVE_TIMEOUT=0; fi

# self-heal: web interface reachable? re-assign usb0 IP; if still down, restart xochitl once.
# NOTE: this cannot repair WebInterfaceEnabled=false in xochitl.conf -- xochitl owns that
# file and rewrites it from memory, so it must be set with xochitl STOPPED. If the interface
# stays down across runs, check that key first (see docs/phase-0-findings.md).
if ! web_up; then
  ip addr add 10.11.99.1/32 dev usb0 2>/dev/null
  ip link set usb0 up 2>/dev/null
  sleep 2
  if ! web_up; then
    log "restarting xochitl to bind web interface"
    systemctl restart xochitl 2>/dev/null
    sleep 12
  fi
fi

if ! web_up; then
  log "ABORT: web interface down at $WEB (nothing rendered, nothing pushed)"
  log "checked=0 pushed=0 failed=0 aborted=1 done $(date)"
  exit 0
fi

checked=0; pushed=0; failed=0; skipped=0
last_md5=""

for meta in "$XDIR"/*.metadata; do
  [ -e "$meta" ] || continue
  uuid=$(basename "$meta" .metadata)
  grep -q '"type": *"DocumentType"' "$meta" || continue
  grep -q '"deleted": *true' "$meta" && continue
  grep -q '"parent": *"trash"' "$meta" && continue

  # change key = lastModified value (isolated so it can't grab createdTime/lastOpened);
  # md5 fallback if the field is missing/unparseable.
  key=$(grep -o '"lastModified"[^0-9]*[0-9][0-9]*' "$meta" 2>/dev/null | grep -oE '[0-9]+' | head -n 1)
  [ -z "$key" ] && key=$(md5sum "$meta" 2>/dev/null | cut -d' ' -f1)
  [ -z "$key" ] && key=none

  checked=$((checked+1))
  prev=$(awk -v u="$uuid" '$1==u{print $2}' "$MAN" | head -n 1)
  [ "$prev" = "$key" ] && continue

  if ! render_one "$uuid"; then
    failed=$((failed+1))
    # Distinguish "this one document failed" from "the interface died under us". The
    # latter made v4 emit 18 identical failures per run; stop at the first sign of it.
    if ! web_up; then
      log "ABORT: web interface went down mid-run after $uuid"
      break
    fi
    continue
  fi

  md5=$(md5sum "$OUT" 2>/dev/null | cut -d' ' -f1)
  if [ -n "$last_md5" ] && [ "$md5" = "$last_md5" ]; then
    # (4) byte-identical to the render before it: the v4 bug's exact signature. Refuse to
    # push rather than risk filing one notebook's content under another notebook's uuid.
    log "SUSPECT-DUPLICATE $uuid md5=$md5 identical to previous render -- NOT pushed"
    skipped=$((skipped+1))
    continue
  fi

  push_one "$uuid" "$key" "$sz"
  prc=$?                                         # capture before anything else clobbers $?
  if [ $prc -eq 0 ]; then
    # manifest written only now -- a failed push must leave the doc pending for next run
    awk -v u="$uuid" '$1!=u' "$MAN" > "$MAN.t" 2>/dev/null
    echo "$uuid $key $md5" >> "$MAN.t"
    mv "$MAN.t" "$MAN"
    last_md5=$md5
    pushed=$((pushed+1))
    log "pushed $uuid sz=$sz md5=$md5"
  else
    failed=$((failed+1))
    log "push-fail $uuid sz=$sz md5=$md5 rc=$prc (left pending)"
  fi
done

log "checked=$checked pushed=$pushed failed=$failed skipped=$skipped done $(date)"
rm -f "$PKT" "$OUT"
