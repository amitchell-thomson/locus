# keryx archives Gmail mail itself: labels over IMAP, not moves through Mail

**State:** done 2026-10-28 · **Written:** 2026-10-28 · **Part of:** [keryx-joins-locus.md](keryx-joins-locus.md) (round 11)
**From:** your words, 28 Oct: "lets do that" (stop archiving on Personal now, and a spec for the
direct route), after "try connection doctor before setting up anything new".

**Done when:** `a` and `d` on Personal and OQTS take a thread out of the inbox and it stays out, keryx
sends Gmail nothing but label changes (no upload, no delete, no expunge, provably), and
`archives = false` comes off both Gmail accounts.

---

## Why

Mail's Connection Doctor log, 28 Oct, one archive of a newsletter on Personal:

```
APPEND "[Gmail]/All Mail" … {57250}      ← Mail uploads a fresh copy (new UID 10720)
UID STORE 10717 +FLAGS.SILENT (\Deleted)  ← and deletes the original in All Mail
UID EXPUNGE 10717
FETCH … UID 10720  X-GM-LABELS ("\\Inbox")  ← Gmail: the copy is still in the inbox
```

So keryx's archive on Personal never removed the Inbox label, and it sent Gmail a delete in your
name. Only a duplicate went, with Gmail's settings as they are, but keryx's rule is that it never
deletes. Auto-Expunge made no difference. On OQTS the same AppleScript request became Gmail's own
label command to put a thread back (`UID STORE +X-GM-LABELS (\Inbox)`), but an archive there was
only recorded in Mail's own index: after a forced sync Gmail still listed the test mail in the
inbox. So neither Gmail account really archives through Mail. Both are now `archives = false`:
nothing leaves their inboxes through keryx, and marks and needs-reply still work.

## What changes

keryxd talks IMAP to `imap.gmail.com` itself for **label changes only**, on accounts that opt in
(`[[accounts]] gmail_imap = true`; Personal and OQTS). Everything else stays as it is: Mail still
syncs, shows and sends the account, keryx still reads the mirror from Mail's store and waits for
Mail's index to show the change (`await_envelope`), and sends still go through Mail.

| keryx action | IMAP, in `[Gmail]/All Mail` |
| --- | --- |
| archive, file (option C) | `UID STORE <uid> -X-GM-LABELS (\Inbox)` |
| back to the inbox | `UID STORE <uid> +X-GM-LABELS (\Inbox)` |
| read / unread, flag | unchanged: through Mail as now |

The message is found by its Message-ID (`UID SEARCH HEADER Message-ID <id>`), never by Mail's
ROWID or a remembered UID, since Mail's own moves renumber them.

**Never delete, by construction.** The IMAP client offers one function, `set_inbox(msg, on)`.
There is no code path that sends `APPEND`, `\Deleted`, `EXPUNGE`, `MOVE`, `COPY` or a label other
than `\Inbox`, and a test checks every command the client can emit against that list.

**Sign-in: an app password**, made at myaccount.google.com → Security → App passwords (it needs
2-step verification), kept in the Keychain as `keryx-imap-<account>`. No OAuth app and no public
pages. `keryx doctor` checks it logs in and finds `[Gmail]/All Mail`.

## Steps

1. **I1:** the IMAP client in keryxd (a small, synchronous client over TLS; crate chosen at the
   step and named in the report), `set_inbox` only, and the command-list test. Fixture-tested
   against a fake server.
2. **I2:** actuate routes inbox-label changes for `gmail_imap` accounts to it; the rest stays on
   AppleScript. `archives = false` still wins if both are set.
3. **I3:** `keryx doctor` + `docs/setup.md` (making the app password, the Keychain entry).
4. **I4 (you):** the app password. Then live on `[keryx-test]` mail, with Connection Doctor
   logging on: archive, back, file. The log must show only `X-GM-LABELS` for keryx's changes and
   the thread must stay where it was put after Mail's next sync. Then `archives = false` comes
   off Personal and OQTS.

## Open

- **OQTS shared** is read-only in keryx and stays out of this. OQTS needs its own app password.
- **Undoing the earlier archives.** Five Personal threads came back on 27–28 Oct (Barchart,
  Netflix, LinkedIn, a GitHub run, Bloomberg). After I4 you can archive them again with `a`.

## Progress

- **I1–I3 done, 28 Oct.** No IMAP crate: `keryx-store/src/gmail_imap.rs` is a hand-written client
  over rustls (already in the workspace through reqwest) with the Mac's certificate store, so its
  six commands are all it can send (LOGIN, LIST, SELECT, UID SEARCH X-GM-RAW, UID STORE
  ±X-GM-LABELS (\Inbox), LOGOUT). Tests against a fake Gmail: every copy of a Message-ID
  changes, other labels stay, All Mail is found by `\All` under another name, a wrong password /
  a missing message / a label Gmail keeps are errors, and no other command is possible.
  `actuate` sends only the Inbox label over IMAP and leaves flags, read state and other labels to
  Mail; after the change it waits for Mail to show it and nudges Mail to sync once if it is slow.
  `keryx doctor` signs in and names All Mail. Checked against the real Gmail with a wrong
  password: TLS, greeting and LOGIN work, and the refusal reads "sign-in refused … check the app
  password".
- **I4 done, 28 Oct.** App passwords for Personal and OQTS (OQTS needed 2-Step Verification
  allowed in its Admin console first). `keryx doctor` signs in on both. Round trips through
  keryxd: a Personal `[keryx-test]` mail archived in 1.6 s and shown by Mail 8 s later on the same
  UID (no copy); Gmail's own report, in Mail's log, lists it without `\Inbox`, and it stayed out.
  On OQTS, a fresh message archived and came back into the inbox within 3 s of each change.
  Mail's log shows no `APPEND` or `EXPUNGE` from keryx's changes. `archives = false` is off both.
- **Mail only re-reads what changed since its last checkpoint** (`UID FETCH … CHANGEDSINCE`).
  The OQTS test mail whose archive Mail recorded on 27 Oct and never sent kept showing as
  archived in Mail while Gmail had it in the inbox: nothing about it had changed at Gmail since
  Mail's checkpoint, so Mail never asked, and a later `+X-GM-LABELS (\Inbox)` that Mail did hear
  about did not change what it showed. It was cured by making Gmail match (one label change).
  Any other mail archived through Mail on OQTS before 28 Oct may be in the same state: archived in
  Mail and keryx, still in the inbox at Gmail (the phone and the web show it there).
- **It is slow per thread** (~1.5 s at Gmail, then up to ~15 s while keryx waits for Mail to
  show it). The TUI doesn't wait (the background worker), but a 13-thread `keryx archive` outlived
  the CLI's patience while keryxd carried on and finished it.

