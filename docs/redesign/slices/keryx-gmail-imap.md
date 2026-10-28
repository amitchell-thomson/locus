# keryx archives Gmail mail itself: labels over IMAP, not moves through Mail

**State:** draft for approval · **Written:** 2026-10-28 · **Part of:** [keryx-joins-locus.md](keryx-joins-locus.md) (round 11)
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
