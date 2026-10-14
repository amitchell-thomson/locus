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
   - *Branding*: app name `keryx`, your Gmail as support and developer contact.
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

ntfy app → subscribe to the topic printed by `keryx doctor` (stored as `keryx-ntfy-topic`), on `https://ntfy.sh`. Your iPhone is already on Tailscale.

## 9. Scheduled runs (Phase 9)

`keryx doctor` installs nothing by itself. Phase 9 adds `scripts/install-runs.sh`, which writes the launchd agent for the 08:00/18:00 runs. You run it once.
