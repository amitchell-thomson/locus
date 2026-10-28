# Setting up keryx (Alec's steps)

Each step says which phase needs it. Nothing here goes in the repo: secrets live in the macOS Keychain.

## 1. Config (Phase 1)

```sh
mkdir -p ~/.config/keryx
cp ~/local-projects/keryx/config/config.example.toml ~/.config/keryx/config.toml
keryx doctor            # names every problem in the config
```
The example already has your four accounts and the Mail.app account ids found on 2026-10-14. Leave `live = false` until the Phase 4 checks pass.

## 2. Gmail (one app password per account, for archiving)

Personal Gmail goes through Mac Mail, like the OQTS accounts (Alec, 2026-10-20): no Google Cloud project or OAuth. The Gmail API backend still exists (`backend = "gmail"` + `keryx auth gmail`) if it's ever wanted; it needs a published OAuth app with public home and privacy pages.

**Archiving needs IMAP** (spec `docs/redesign/slices/keryx-gmail-imap.md`, 28 Oct). Mail cannot archive these accounts for keryx: on Personal it uploaded a copy and deleted the original, on OQTS it never told Gmail. So keryxd changes Gmail's Inbox label itself, and nothing else. For each Gmail account (Personal, OQTS):

1. At <https://myaccount.google.com/apppasswords> (signed in as that account; it needs 2-Step Verification on), make an app password named `keryx`. Google shows 16 letters in four groups.
2. In a terminal, store it; `security` asks for it, so it never appears in your shell history:
   ```sh
   security add-generic-password -s keryx-imap-personal -a alec.mitchellthomson@gmail.com -w
   security add-generic-password -s keryx-imap-oqts -a alec@oqts.org -w
   ```
   (The spaces Google shows can be typed or left out.)
3. In `~/.config/keryx/config.toml`, on that account: add `gmail_imap = true`.
4. `keryx doctor` must say `Gmail IMAP signs in as … (All Mail is "[Gmail]/All Mail")`.
5. Restart keryxd (`deploy/keryx/install-daemon.sh`), check a round trip on a `[keryx-test]` mail, then delete the account's `archives = false` line.

To stop using it, take `gmail_imap = true` out (and put `archives = false` back), and revoke the app password on the same Google page.

## 3. Mail.app permissions for keryxd (Phase 1)

First make keryx's API token (keryxd won't start without it), then install keryxd as a launchd agent (starts at login, restarts if it stops):
```sh
security add-generic-password -U -s keryx-api-token -a "$USER" -w "$(openssl rand -hex 32)"
cd ~/local-projects/locus && deploy/keryx/install-daemon.sh
```

keryxd reads Mail's store and acts through AppleScript, so it needs two macOS permissions of its own (your terminal's don't carry over to a launchd agent):

- **Full Disk Access**: System Settings → Privacy & Security → Full Disk Access → `+` → `⇧⌘G` → `~/.cargo/bin/keryxd` → on.
- **Automation → Mail**: the first time keryxd moves a message, macOS asks "keryxd wants to control Mail". Allow it. (`keryx doctor` triggers this on purpose with a harmless script.)

Mail must be signed in to Personal Gmail, Oxford, OQTS and OQTS shared (it already is). keryxd starts Mail in the background when it needs it.

Phase 1 checks whether these grants survive `cargo install` rebuilds. If they don't, `scripts/sign.sh` will sign keryxd with a stable local identity, and you grant once more.

## 4. Test messages and the Mail.app check (Phase 1)

From your phone or webmail, send yourself one email at each Mail account (newc7370@ox.ac.uk, alec@oqts.org, oqts@oqts.org) and one at your Gmail, each with a subject starting **`[keryx-test]`** and a small PDF attached. Then:
```sh
deploy/keryx/verify-mailapp.sh
```
It moves, flags and replies to *only* those test messages, and prints what worked.

## 5. API token (Phase 4)

```sh
security add-generic-password -U -s keryx-api-token -a "$USER" -w "$(openssl rand -hex 32)"
```
kairos's token is already in the Keychain as `kairos-api-token`, and keryx reuses it to talk to kairos.

## 6. Claude Code (Phase 5)

`docs/claude-usage.md` will have the one `claude mcp add` command. Also turn off the **claude.ai Gmail connector** (claude.ai → Settings → Connectors → Gmail → disconnect), since keryx replaces it and that connector can send without approval.

## 7. Neovim address completion (Phase 6)

`docs/nvim.md` will have a small snippet for your config (chezmoi). keryx never edits your dotfiles.

## 8. Phone

Removed on 26 Oct 2026 (keryx-joins-locus X2, plan B.4): no ntfy pushes and no approval pages.
Drafts and proposals are approved in locus's Approve; the phone shows your mail through Mail.

## 9. Scheduled runs (Phase 9)

1. Claude Code must be installed and logged in (`claude --version`). Runs use your subscription.
2. Try one by hand: `keryx run morning --dry-run` (prompt + tool list), then `keryx run morning`. It shows in the TUI under `g` → `c`.
3. Install the 08:00 / 18:00 agent: `deploy/keryx/install-runs.sh` (other times: `KERYX_MORNING=07:30 KERYX_EVENING=18:30 deploy/keryx/install-runs.sh`, and set the same in `[runs]`). A time missed while the Mac sleeps runs once on wake.
4. Background cleanup whenever you like: `keryx cleanup` (all accounts) or `keryx cleanup --account oxford "be strict with society newsletters"`; review with `A`.
5. Logs: `~/Library/Logs/keryx-runs.log`. Turn runs off with `[runs] enabled = false`.

A run's Claude has only keryx's tools for that run (no shell, files, web or kairos), its token works only for that run and only for MCP, and it can't send, delete or unsubscribe.
