# keryx — notes for Claude

Terminal email client (ratatui) with Claude access over MCP, for Alec's four accounts. Spec: `SPEC.md`. Second app in Alec's TUI ecosystem; **kairos** (`~/local-projects/kairos`) is the reference architecture: when in doubt, do what it does. Alec checks work through tests and acceptance criteria, so every semantic rule has a test.

## Layout

| crate | role |
|---|---|
| `keryx-core` | Pure logic, no network/SQLite. `config`, `model`, `mime` (→ normalised text), `text` (HTML hygiene, terminal sanitising), `thread`, `reply` (recipients, quoting, subjects), `query` (search syntax), `risk` (§6 policy), `changeset`, `applescript` (templates + `quote`), `api` (shared request/response types). |
| `keryx-store` | Mirror DB (SQLite WAL + FTS5; only keryxd writes it, everyone else opens it read-only), `mailapp` reader (Envelope Index `mode=ro`, `.emlx`, `.partial.emlx`), `gmail` client (OAuth refresh, full + history sync, actions), `kairos` client, `client` (blocking keryxd client). |
| `keryxd` | Single writer, on the Mac. `engine` (intents → changesets → risk → apply / approve / reject / undo; send hold), `http` (axum + signed phone links), `mcp` (rmcp, stateless Streamable HTTP), `notify` (ntfy), `jobs` (Gmail history poll, Mail store watch, snooze wake-ups, hold queue), `actuate` (Gmail API, osascript). |
| `keryx-cli` | `keryx` binary: bare = TUI; `inbox`, `search`, `show`, triage commands, `reply`, `changes`, `approve`, `doctor`, `auth gmail`, `--json`. |
| `keryx-tui` | ratatui front end (`app` = state/keys, testable; `ui` = drawing). Theme copied from kairos-tui. |
| `xtask` | `cargo xtask mailapp-fixture` (synthetic Envelope Index + emlx tree from the real schema). |

Data flow: Gmail API → keryxd → `~/.local/share/keryx/keryx.db`. Mail.app (Oxford, OQTS-Alec, OQTS shared) → `~/Library/Mail/V10` (read-only) → keryxd → DB. TUI/CLI read the DB; all writes go to keryxd HTTP (`<mac tailscale ip>:7421`) → source (Gmail API / AppleScript) → mirror updated from the source's confirmation. kairos at `compute-node:7420` for tasks and free time.

## Invariants (don't break these)

- **Single writer**: only keryxd writes the mirror or acts on mailboxes.
- **Claude can never send.** Every send by a non-interactive author is high-risk (pending). There's no approve, send, forward or delete tool in MCP. `draft_reply` recipients are computed server-side.
- **Email is untrusted data** (SPEC §5): MCP text goes only in `email_data` envelopes with a per-response nonce; HTML hidden text is stripped and flagged; control/escape sequences never reach a terminal; email strings enter AppleScript only via `applescript::quote`.
- **Risk is computed, never supplied** (`risk::assess`; order forbidden → high → low). Alec's interactive edits (`x-keryx-author: alec` + `x-keryx-interactive: 1`) get forbidden rules only. A missing author header means `script` (stricter than kairos).
- **Never delete**: no code path trashes, expunges or marks spam. Gmail scope is `gmail.modify` only, never `https://mail.google.com/`.
- **Every write is a changeset** with pre/post images. Apply checks preconditions; undo refuses with a diff if the target moved since.
- **Sends hold 10 s in keryxd** (cancellable with `u` / `keryx cancel`) after Alec's confirm or approval. Preconditions are re-checked at send time.
- **`live = false` by default**: writes only to `[keryx-test]` messages; sends only to Alec's own addresses. Checked at propose *and* apply.
- **People mail is Alec's**: Claude's triage applies at once only to *automated* threads (computed from headers by `keryx-core::automated`, never claimed by the caller; never automated if Alec has sent to the sender). Mixed batches are split: automated applied, the rest become cleanup proposals.
- **Cleanups are proposals until Alec decides** (archive only, no delete): every proposal in a review waits for him, even automated mail. Accepting is his interactive action; stale proposals are never applied by accept-all. Claude has no accept tool. There are no pending *triage* changesets: anything needing Alec becomes a proposal. Pending changesets are for sends only.
- **Only Alec writes to kairos through keryx**: Claude makes *suggestions* (task / deadline / event) that Alec accepts by saving a prefilled form. No MCP tool reaches kairos. keryx always sends the true author header (`alec`+interactive or `script`).
- **Scheduled runs are contained by their tool list**: `claude -p --strict-mcp-config` with keryx tools only (no Bash, web, file or kairos), a run-scoped token, one push per run, and everything logged in the Runs view.
- Attachment text for Claude is extracted by keryxd (PDF / DOCX / text), capped, and envelope-wrapped. Raw bytes never go to Claude, and nothing is executed.
- **Filing is by shared key** (Admin is Admin on any account), and always takes the thread out of the inbox.
- No `unwrap()`/`expect()` outside tests (workspace clippy lints deny them). `thiserror` in libs, `anyhow` in bins.

## Running things

```sh
scripts/ci.sh                 # fmt --check, clippy -D warnings, all tests, perf budgets (release)
cargo test -p keryx-core      # semantics: MIME, hygiene, threading, reply-all, query, risk table
cargo test -p keryxd          # engine end-to-end, HTTP, MCP (+ restart mid-session), mock kairos
cargo test -p keryx-tui       # insta snapshots; review with `cargo insta review`
cargo xtask mailapp-fixture   # rebuild fixtures/mailapp (synthetic Mail store) from fixtures/eml
cargo run -p keryx-tui --example frame -- <config> <rfc3339-now> <w> <h> [keys]   # one TUI frame as text (real mirror: don't paste it anywhere)
cargo test --release -p keryx-cli --test perf -- --nocapture                      # 10k-message budgets
```
Commit only when `scripts/ci.sh` **exits 0** — check its exit status, never a grep of its output (a piped grep once hid a clippy failure and a red commit was pushed, 5c1ce18; fixed in the next commit).
Use `/bin/ls` in scripts: Alec's shell aliases `ls` to eza, which can hang a non-interactive command.
`KERYX_NOW=<rfc3339>` fixes "now"; `KERYX_CONFIG` overrides the config path.

## Shared-crate candidates (with kairos; don't extract until needed twice)

Extracted so far: **eco-dates** (`~/local-projects/eco`, private repo amitchell-thomson/eco; kairos's switch waits on branch `shared-dates`). Dependencies are git + pinned `rev`; `.cargo/config.toml` sets `net.git-fetch-with-cli`.


Tracked in SPEC §11: changeset/risk/approval engine, ntfy + signed links, stateless MCP setup, TUI kit (theme, key hints, forms, confirm, approvals pane), `kairos-core::dates`, `app://` cross-links (`keryx://thread/<account>/<id>`), Claude popup (deferred; do it the same way across apps), MCP gateway + one approval queue. When copying code from kairos, add a line here naming the source file.

## Phase status

| Phase | Status |
|---|---|
| 0 Scaffolding & setup | Done 2026-10-14: workspace, lints, ci.sh, config schema + 16 validation tests, docs/setup.md, fixtures (26 .eml incl. PDF/DOCX/corrupt attachments, Gmail API JSON, synthetic Mail store via `cargo xtask mailapp-fixture`). Encrypted-PDF fixture deferred to Phase 2. |
| 1 Read plumbing & verification | Code done 2026-10-14: mirror DB (WAL, FTS5, batches), Mail.app reader + ingest (incl. header-only for bodies Mail hasn't downloaded, index-rebuild detection, interrupted-pass convergence), Gmail full/history sync + OAuth loopback/PKCE (fixture-tested), keryxd `--once` + watch/poll loop, `keryx doctor [--discover]`, `keryx auth gmail`, launchd agent + `scripts/install-daemon.sh`, `scripts/verify-mailapp.sh`. Real store: 3,535 messages mirrored in 1.5 s (release), re-pass 0.1 s, every Mail mailbox count matches. **Waiting on Alec:** Gmail OAuth client + `keryx auth gmail`; `scripts/verify-mailapp.sh` with `[keryx-test]` mails; `scripts/install-daemon.sh` + FDA grant. |
| 2 Core semantics | Done 2026-10-14: `text` (HTML hygiene, sanitising, compact; TEXT_VERSION 2 + `ingest::rederive` from stored raw at keryxd start), `automated`, `unsubscribe`, `invite` (TZID incl. Windows names → London), `thread` (References fallback, no subject merging), `reply` (recipients across all own addresses/aliases, From account, subjects, quote, forward), `query`, `display`, `applescript::quote` (proptest), `attach` (PDF/DOCX/text/HTML with caps, 10 s budget, catch_unwind). 87 core tests incl. proptests + an all-fixtures render snapshot. |
| 3 CLI + TUI read-only | Done 2026-10-14: `keryx-store::view` (thread rows per view, search, address book, `sent_to`), `keryx-store::testkit` (feature `testkit`: fixture mirror + fake Gmail), TUI (list, thread with folded quotes / invite cards / ⚠ hidden text / attachments, reading pane ≥ 120 cols, Tab accounts, g go-to, live / search, help, stale-sync status only), CLI `inbox`/`search`/`show`/`addresses` + `--json` (docs/json.md) + `@N`, bare `keryx` = TUI, `frame` example. 15 TUI snapshot tests, 5 CLI integration tests. Budgets (10k msgs, release): inbox 5 ms, search 17 ms, first frame 9 ms. |
| 4 Write path (triage + cleanup reviews) | Code done 2026-10-14: core `changeset`/`risk`/`location`; store `state` (changesets, snoozes, tags, reviews, proposals + fingerprints), `api`, `client`; keryxd `engine` (triage → split applied/proposals, preconditions + live guard at apply, rollback, undo with diff, snooze wake on time/reply, tags, reviews), `actuate` (Gmail modify; Mail argv-only AppleScript; Gmail-over-IMAP never moves out of All Mail), `http` (axum, bearer, author headers); TUI keys d s f m U u x N A + review screen; CLI archive/unarchive/flag/unflag/read/unread/snooze/file/undo/changes/sync; eco-dates shared crate. **Waiting on Alec** for the live ACs: keryx-api-token, install-daemon + FDA/Automation, then `[keryx-test]` round trips (≤ 10 s to Gmail web / Mac Mail / iPhone; Mail-not-running start). |
| 5 MCP (read, triage, cleanups) | Code done 2026-10-14: keryxd `mcp` (rmcp, stateless, behind the bearer token at /mcp): read / triage / tags / propose_cleanup / reviews / feedback / changesets / undo; no send/approve/accept/forward/unsubscribe/delete tools; every result in a nonce'd `email_data` envelope with marker brackets neutralised; `keryx_core::when` (shared natural-time parsing). 9 MCP tests (tool list, injection corpus, attachments, policy, propose validation, style samples, restart mid-session, token). docs/claude-usage.md, `.claude/skills/keryx`. **Alec:** register MCP + turn off the claude.ai Gmail connector + the end-to-end Claude Code check. |
| 6 Compose & send (+ draft tools) | Code done 2026-10-14: core `compose` ($EDITOR file) + `outgoing` (mail-builder, warnings, URL list), Send/Unsubscribe ops (no inverse), risk (live=false → own addresses only); engine `engine_send` (send/draft_reply/draft_email, hold → process_holds re-checks, approve/reject/cancel/replace, unsubscribe); actuators (Gmail messages.send; Mail.app send script); HTTP + MCP draft tools; TUI r/R/F/c/L, confirm + approval screens, hold status, `u` cancels; CLI reply/reply-all/forward/compose/approve/reject/cancel/unsubscribe; docs/nvim.md. **Alec:** live send round trips from each account (threading, attachments intact, `u` within the hold), nvim snippet. |
| 7 kairos link (+ suggestion tools) | Not started |
| 8 Phone | Not started |
| 9 Scheduled runs + background cleanups | Not started |

## Environment findings (2026-10-14)

- Mail.app (`~/Library/Mail/V10`) has Personal Gmail `1FD73A3D-…` (ignored by keryx: Gmail API instead), Oxford EWS `74EA59BE-…`, OQTS-Alec `09F30390-…`, OQTS shared `428452FB-…`; account ids → addresses via `~/Library/Accounts/Accounts4.sqlite` (`ZACCOUNT` parent rows).
- **Gmail-over-IMAP accounts in Mail (both OQTS)**: every message lives physically in `[Gmail]/All Mail`; `INBOX`, `Sent Mail`, `Important`, `Starred` are *virtual* mailboxes (`mailboxes.source` = All Mail's ROWID) with membership in `labels(message_id, mailbox_id)`. Exchange (Oxford) uses real mailboxes (`messages.mailbox`). The reader must take location = physical mailbox ∪ label mailboxes.
- `.emlx` path: `<acct>/<a>.mbox/<b>.mbox/<store-uuid>/Data/<digits of ROWID/1000, least significant first>/Messages/<ROWID>[.partial].emlx` (`Data/Messages/` below 1000); attachments of partial ones under `Data/Attachments/<ROWID>/<part>/<filename>`. emlx = byte-count line, message, plist (`flags`, `remote-id`, dates).
- `messages.global_message_id` → `message_global_data.ROWID` (has `message_id_header`); `recipients.type` 0 = To, 1 = Cc; dates are Unix seconds; `subject_prefix` holds "Re: " etc. separately.
- `messages.automated_conversation` takes 0 / 1 / 2 (2,731 / 220 / 4,379 of 7,330): meaning unknown, so keryx's automated detection relies on headers, not this column (Phase 2 decides whether 1 is a usable hint).
- ~7.5k `.emlx`, ~3.5k of them `.partial.emlx`.
- Some messages have no body on disk yet (13 Oxford rows had no `.emlx`; one OQTS `.emlx` was a 0-byte placeholder): Mail downloads lazily. keryx mirrors them header-only from the Envelope Index (`text_version` 0) and re-tries every pass.
- Raw 8-bit (non-RFC 2047) headers exist in the wild: subjects are re-decoded with the message charset, else Windows-1252 (`mime::fix_8bit`; TEXT_VERSION 3).
- Text v2 on real mail: 94 / 3,535 messages had hidden text removed (mostly newsletter preheaders: the ⚠ may prove noisy — revisit after Alec uses the TUI); 134 have no text body (14 not downloaded, the rest image/calendar/attachment-only).
- `pgrep -x Mail` tells whether Mail is running; keryxd will `open -gja Mail` before acting (Phase 4). Envelope Index tables used: `messages`, `mailboxes`, `subjects`, `addresses`, `recipients`, `message_global_data`, `conversations`.
- The terminal has Full Disk Access; keryxd will need its own FDA + Automation grants (check they survive rebuilds in Phase 1).
- The claude.ai Gmail connector in Claude Code can send directly, bypassing keryx: Alec is turning it off (2026-10-14).

## Decisions log

- 2026-10-14 (Alec): name **keryx**; Gmail via the **Gmail API** (OAuth, `gmail.modify`, consent screen in production); all four accounts in v1; daemon **on the Mac only**; kairos-style keys (`d` archive, `s` snooze, `f` flag, `m` file, `x` batch, `t` task, `r`/`R` reply, `w` free times, `c` compose, `A` approvals, `Tab` accounts); own sends = confirm screen + 10 s undo window; Claude = **MCP only** for now (TUI popup later, consistent across kairos and keryx); filing by **shared keys** across accounts.
- 2026-10-14 capabilities (Alec):
  - Claude may triage **automated mail only** without approval;
  - **morning + evening runs** (08:00 / 18:00): clear automated mail, tag needs-reply, draft replies, find tasks;
  - Claude may read **attachment text** and **Alec's sent mail for tone**;
  - task/deadline finds are **suggestions Alec accepts**, never created directly;
  - one inbox list by date;
  - compose: attachments, per-account signatures, address completion (Neovim);
  - reading: invites → kairos, one-key unsubscribe (Alec only), mark read on open;
  - OQTS shared is used with one other person, so every Claude write there needs approval;
  - no out-of-TUI awareness (tmux, pushes for new mail, summary API) until the ecosystem **home page** exists.
- 2026-10-14 cleanups (Alec): a **primary use case** is Claude proposing archive/file/snooze/keep for every thread, decided item by item in the TUI, so the inbox holds only new / needs-action / needed-soon mail. **Archive only** (no delete or trash). In a cleanup, everything is reviewed, including automated mail. MCP moved up to Phase 5.
- 2026-10-14 go-ahead (Alec): build straight through all phases (report per phase, stop only for him); shared crate for natural dates; private GitHub repo `amitchell-thomson/keryx`; all SPEC §13 defaults accepted.
