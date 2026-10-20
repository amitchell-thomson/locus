# Setting up keryx (Alec's steps)

Each step says which phase needs it. Nothing here goes in the repo: secrets live in the macOS Keychain.

## 1. Config (Phase 1)

```sh
mkdir -p ~/.config/keryx
cp ~/local-projects/keryx/config/config.example.toml ~/.config/keryx/config.toml
keryx doctor            # names every problem in the config
```
The example already has your four accounts and the Mail.app account ids found on 2026-10-14. Leave `live = false` until the Phase 4 checks pass.

## 2. Gmail API client (Phase 1, about 10 minutes)

keryx talks to your personal Gmail through the Gmail API with its own OAuth client. Only you use it.

1. <https://console.cloud.google.com/> → project picker → **New project**, name it `keryx`.
2. **APIs & Services → Library** → search **Gmail API** → **Enable**.
3. **Google Auth Platform** (older consoles: *OAuth consent screen*):
   - *Branding*: app name `keryx`, your Gmail as support and developer contact. Publishing to production needs an **application home page** and a **privacy policy** URL: publish the two small pages in `docs/oauth-site/` (e.g. GitHub Pages at `https://amitchell-thomson.github.io/keryx-oauth/`), use `…/` and `…/privacy.html`, and add `amitchell-thomson.github.io` under **Authorised domains**. Leave terms of service empty. You don't submit for verification.
   - *Audience*: user type **External**, then **Publish app** so the status reads **In production**. This matters: in *Testing*, Google expires the refresh token every 7 days and keryx would lose access weekly. "Unverified" is fine for an app only you use.
   - *Data access*: add the scope `https://www.googleapis.com/auth/gmail.modify` (read, label, draft and send; it **can't** permanently delete). Don't add `https://mail.google.com/`.
4. *Clients* → **Create client** → type **Desktop app**, name `keryx` → **Download JSON**.
5. Put it in the Keychain, then delete the download:
   ```sh
   security add-generic-password -U -s keryx-gmail-client -a "$USER" -w "$(cat ~/Downloads/client_secret_*.json)"
   rm ~/Downloads/client_secret_*.json
   ```
6. `keryx auth gmail` opens the browser. Google warns *"Google hasn't verified this app"* → **Advanced** → **Go to keryx (unsafe)** → allow. keryx stores the refresh token as `keryx-gmail-refresh` in the Keychain.

## 3. Mail.app permissions for keryxd (Phase 1)

Install keryxd as a launchd agent (starts at login, restarts if it stops):
```sh
cd ~/local-projects/keryx && scripts/install-daemon.sh
```

keryxd reads Mail's store and acts through AppleScript, so it needs two macOS permissions of its own (your terminal's don't carry over to a launchd agent):

- **Full Disk Access**: System Settings → Privacy & Security → Full Disk Access → `+` → `⇧⌘G` → `~/.cargo/bin/keryxd` → on.
- **Automation → Mail**: the first time keryxd moves a message, macOS asks "keryxd wants to control Mail". Allow it. (`keryx doctor` triggers this on purpose with a harmless script.)

Mail must be signed in to Oxford, OQTS and OQTS shared (it already is). keryxd starts Mail in the background when it needs it.

Phase 1 checks whether these grants survive `cargo install` rebuilds. If they don't, `scripts/sign.sh` will sign keryxd with a stable local identity, and you grant once more.

## 4. Test messages and the Mail.app check (Phase 1)

From your phone or webmail, send yourself one email at each Mail account (newc7370@ox.ac.uk, alec@oqts.org, oqts@oqts.org) and one at your Gmail, each with a subject starting **`[keryx-test]`** and a small PDF attached. Then:
```sh
scripts/verify-mailapp.sh
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

## 8. Phone (Phase 8)

1. Make a private topic and store it (keryxd reads it at start):
   ```sh
   security add-generic-password -U -s keryx-ntfy-topic -a "$USER" -w "keryx-$(openssl rand -hex 12)"
   security find-generic-password -s keryx-ntfy-topic -w   # the name to subscribe to
   ```
2. ntfy app on the iPhone → subscribe to that topic on `https://ntfy.sh` (separate from kairos's topic, so they group apart).
3. In `config.toml`, `[server] bind` must be the Mac's Tailscale IP (`tailscale ip -4`), so the phone can reach the approval pages. Links are signed with a key derived from `keryx-api-token`; rotating the token invalidates old links.
4. Restart keryxd (`scripts/install-daemon.sh`). `keryx doctor` should say "ntfy topic set".

Approval pushes are **off** (`[notify] approvals = false`, Alec 2026-10-20): drafts are approved in keryx (`A`), the phone only gets the one push per run and per finished cleanup, and the pages show drafts without buttons. With `approvals = true`, each Claude draft pushes with buttons that open a confirmation page; nothing is approved until you press its button (link previews can't approve). If the Mac is asleep the page won't load and the draft stays pending.

## 9. Scheduled runs (Phase 9)

1. Claude Code must be installed and logged in (`claude --version`). Runs use your subscription.
2. Try one by hand: `keryx run morning --dry-run` (prompt + tool list), then `keryx run morning`. It shows in the TUI under `g` → `c`.
3. Install the 08:00 / 18:00 agent: `scripts/install-runs.sh` (other times: `KERYX_MORNING=07:30 KERYX_EVENING=18:30 scripts/install-runs.sh`, and set the same in `[runs]`). A time missed while the Mac sleeps runs once on wake.
4. Background cleanup whenever you like: `keryx cleanup` (all accounts) or `keryx cleanup --account oxford "be strict with society newsletters"`; review with `A`.
5. Logs: `~/Library/Logs/keryx-runs.log`. Turn runs off with `[runs] enabled = false`.

A run's Claude has only keryx's tools for that run (no shell, files, web or kairos), its token works only for that run and only for MCP, and it can't send, delete or unsubscribe.
