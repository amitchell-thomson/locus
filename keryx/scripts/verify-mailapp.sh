#!/usr/bin/env bash
# Phase 1 check: does Mail's AppleScript do what keryx needs, on each Mail
# account? Touches ONLY messages whose subject starts with "[keryx-test]"
# (send yourself one per account first, each with a small PDF attached —
# docs/setup.md §4). Prints one line per check; record the results in
# CLAUDE.md "Environment findings".
#
# Usage: scripts/verify-mailapp.sh [--no-send]
set -uo pipefail

ENVELOPE="$HOME/Library/Mail/V10/MailData/Envelope Index"
SEND=1
[ "${1:-}" = "--no-send" ] && SEND=0

# Mail account name | inbox | archive | your address on it
ACCOUNTS=(
  "Oxford|Inbox|Archive|newc7370@ox.ac.uk"
  "OQTS-Alec|INBOX|[Gmail]/All Mail|alec@oqts.org"
  "OQTS|INBOX|[Gmail]/All Mail|oqts@oqts.org"
)
PASS=0; FAIL=0
ok()   { echo "  ✓ $*"; PASS=$((PASS+1)); }
bad()  { echo "  ✗ $*"; FAIL=$((FAIL+1)); }

# Run AppleScript with arguments (never interpolated into the script text).
as() { local script="$1"; shift; osascript - "$@" <<<"$script" 2>&1; }

envelope() { sqlite3 -readonly "file:$ENVELOPE?mode=ro" "$1" 2>&1; }

# Wait up to 10 s for an Envelope Index query to return the expected value.
await() {
  local q="$1" want="$2" got
  for _ in $(seq 1 20); do
    got=$(envelope "$q")
    [ "$got" = "$want" ] && { echo "$got"; return 0; }
    sleep 0.5
  done
  echo "$got"; return 1
}

echo "Starting Mail…"
open -gja Mail; sleep 3

for spec in "${ACCOUNTS[@]}"; do
  IFS='|' read -r ACC INBOX ARCHIVE ADDR <<<"$spec"
  echo
  echo "== $ACC ($ADDR)"

  found=$(as '
on run argv
  set {accName, boxName} to argv
  tell application "Mail"
    set mb to mailbox boxName of account accName
    set ms to (messages of mb whose subject begins with "[keryx-test]")
    if (count of ms) is 0 then return "none"
    set m to item 1 of ms
    return (id of m as string) & tab & (message id of m)
  end tell
end run' "$ACC" "$INBOX")
  if [ "$found" = "none" ] || [[ "$found" == *error* ]]; then
    bad "no [keryx-test] message in $INBOX ($found) — send one and re-run"
    continue
  fi
  ASID="${found%%$'\t'*}"; MID="${found#*$'\t'}"
  ok "found test message: AppleScript id $ASID, Message-ID <$MID>"

  r=$(as '
on run argv
  set {accName} to argv
  tell application "Mail"
    set a to account accName
    return id of a
  end tell
end run' "$ACC")
  if [[ "$r" =~ ^[0-9A-F-]{36}$ ]]; then
    r2=$(as '
on run argv
  set {aid} to argv
  tell application "Mail"
    return name of (first account whose id is aid)
  end tell
end run' "$r")
    [ "$r2" = "$ACC" ] && ok "account found by id ($r) — keryxd looks accounts up this way" || bad "lookup by id returned: $r2"
  else bad "Mail account id: $r"; fi

  ROWID=$(envelope "SELECT m.ROWID FROM messages m JOIN message_global_data g ON g.ROWID = m.global_message_id WHERE g.message_id_header = '<$MID>' AND m.deleted = 0 LIMIT 1;")
  if [ "$ROWID" = "$ASID" ]; then ok "AppleScript id == Envelope Index ROWID ($ROWID)"
  else bad "AppleScript id $ASID != Envelope ROWID $ROWID (keryx will look messages up by Message-ID)"; fi

  r=$(as '
on run argv
  set {accName, boxName, mid} to argv
  tell application "Mail"
    set ms to (messages of mailbox boxName of account accName whose message id is mid)
    return count of ms
  end tell
end run' "$ACC" "$INBOX" "$MID")
  [ "$r" = "1" ] && ok "lookup by Message-ID works" || bad "lookup by Message-ID returned: $r"

  for flag in true false; do
    as '
on run argv
  set {accName, boxName, mid, v} to argv
  tell application "Mail"
    set m to item 1 of (messages of mailbox boxName of account accName whose message id is mid)
    set flagged status of m to (v is "true")
  end tell
end run' "$ACC" "$INBOX" "$MID" "$flag" >/dev/null
    want=$([ $flag = true ] && echo 1 || echo 0)
    t0=$(date +%s)
    if got=$(await "SELECT m.flagged FROM messages m JOIN message_global_data g ON g.ROWID = m.global_message_id WHERE g.message_id_header = '<$MID>' AND m.deleted = 0 LIMIT 1;" "$want"); then
      ok "flagged=$flag reached the Envelope Index in $(( $(date +%s) - t0 )) s"
    else bad "flagged=$flag not in the Envelope Index after 10 s (got $got)"; fi
  done

  for read in false true; do
    as '
on run argv
  set {accName, boxName, mid, v} to argv
  tell application "Mail"
    set m to item 1 of (messages of mailbox boxName of account accName whose message id is mid)
    set read status of m to (v is "true")
  end tell
end run' "$ACC" "$INBOX" "$MID" "$read" >/dev/null
    want=$([ $read = true ] && echo 1 || echo 0)
    if got=$(await "SELECT m.read FROM messages m JOIN message_global_data g ON g.ROWID = m.global_message_id WHERE g.message_id_header = '<$MID>' AND m.deleted = 0 LIMIT 1;" "$want"); then
      ok "read=$read reached the Envelope Index"
    else bad "read=$read not in the Envelope Index after 10 s (got $got)"; fi
  done

  # Archive, then back to the inbox.
  r=$(as '
on run argv
  set {accName, boxName, archName, mid} to argv
  tell application "Mail"
    set m to item 1 of (messages of mailbox boxName of account accName whose message id is mid)
    move m to mailbox archName of account accName
  end tell
  return "ok"
end run' "$ACC" "$INBOX" "$ARCHIVE" "$MID")
  INBOX_Q="SELECT count(*) FROM messages m JOIN message_global_data g ON g.ROWID = m.global_message_id JOIN mailboxes mb ON (mb.ROWID = m.mailbox OR mb.ROWID IN (SELECT mailbox_id FROM labels WHERE message_id = m.ROWID)) WHERE g.message_id_header = '<$MID>' AND m.deleted = 0 AND mb.url LIKE '%/$INBOX';"
  if [ "$r" = "ok" ] && got=$(await "$INBOX_Q" "0"); then ok "archive (move to $ARCHIVE) left the inbox"
  else bad "archive: $r (inbox count now $got)"; fi
  NEWROW=$(envelope "SELECT m.ROWID FROM messages m JOIN message_global_data g ON g.ROWID = m.global_message_id WHERE g.message_id_header = '<$MID>' AND m.deleted = 0 LIMIT 1;")
  [ "$NEWROW" = "$ROWID" ] && ok "ROWID unchanged by the move ($ROWID)" || echo "  · ROWID changed on move: $ROWID → $NEWROW (keryx re-finds messages by Message-ID)"

  # Un-archive. Gmail over IMAP: COPY into INBOX (adds the label) — moving a
  # message out of All Mail would delete it in Gmail. Exchange: move back.
  if [[ "$ARCHIVE" == "[Gmail]/"* ]]; then VERB=duplicate; else VERB=move; fi
  r=$(as '
on run argv
  set {accName, boxName, archName, mid, verb} to argv
  tell application "Mail"
    set m to item 1 of (messages of mailbox archName of account accName whose message id is mid)
    if verb is "duplicate" then
      duplicate m to mailbox boxName of account accName
    else
      move m to mailbox boxName of account accName
    end if
  end tell
  return "ok"
end run' "$ACC" "$INBOX" "$ARCHIVE" "$MID" "$VERB")
  if [ "$r" = "ok" ] && got=$(await "$INBOX_Q" "1"); then ok "un-archive ($VERB back to $INBOX) works"
  else bad "un-archive: $r (inbox count now $got)"; fi
  if [ "$VERB" = duplicate ]; then
    n=$(envelope "SELECT count(*) FROM messages m JOIN message_global_data g ON g.ROWID = m.global_message_id WHERE g.message_id_header = '<$MID>' AND m.deleted = 0;")
    [ "$n" = "1" ] && ok "still exactly one copy in Gmail after the label round trip" || bad "Gmail copies after round trip: $n (expected 1)"
  fi

  if [ $SEND = 1 ]; then
    r=$(as '
on run argv
  set {accName, boxName, mid} to argv
  tell application "Mail"
    set m to item 1 of (messages of mailbox boxName of account accName whose message id is mid)
    set r to reply m opening window false
    set content of r to "keryx verify: reply body above the quote."
    send r
  end tell
  return "ok"
end run' "$ACC" "$INBOX" "$MID")
    [ "$r" = "ok" ] && ok "reply sent (check: it threads under the test mail, and whether the quote survived)" || bad "reply: $r"

    PDF="$(cd "$(dirname "$0")/.." && pwd)/fixtures/eml/sheet3.pdf"
    [ -f "$PDF" ] || python3 -I -c "import base64,re,sys;s=open(sys.argv[1]).read();b=re.search(r'base64\n\n(.*?)--m1--',s,re.S).group(1);open(sys.argv[2],'wb').write(base64.b64decode(b))" "$(dirname "$PDF")/23-pdf-attachment.eml" "$PDF"
    r=$(as '
on run argv
  set {addr, pdfPath} to argv
  tell application "Mail"
    set msg to make new outgoing message with properties {subject:"[keryx-test] new message with attachment", content:"keryx verify: new message.", visible:false}
    tell msg
      set sender to addr
      make new to recipient at end of to recipients with properties {address:addr}
    end tell
    tell content of msg to make new attachment with properties {file name:(POSIX file pdfPath)} at after last paragraph
    delay 2
    send msg
  end tell
  return "ok"
end run' "$ADDR" "$PDF")
    [ "$r" = "ok" ] && ok "new message with attachment sent from $ADDR to itself (check the PDF arrives intact)" || bad "new message: $r"
  fi
done

echo
echo "Also check by hand:"
echo "  · Rebuild keryxd (cargo install --path crates/keryxd) and see if 'keryx doctor' still passes"
echo "    under launchd — i.e. Full Disk Access + Automation survive a rebuild."
echo
echo "$PASS passed, $FAIL failed"
[ $FAIL = 0 ]
