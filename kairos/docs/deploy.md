# Deploying kairos

```
iPhone / Calendar.app ─┐
                 iCloud (truth)
        ┌───────────┴────────────┐
 vdirsyncer rw (compute-node)   vdirsyncer read-only (Mac, launchd every 3 min)
        │                        │
 kairosd :7420 (Tailscale)  ◄── kairos CLI / TUI writes (HTTP + bearer)
        ▲  MCP /mcp  ◄── Claude Code
        ▲  /api/tasks ◄── iOS Shortcut
```

## compute-node (server) — already done on 2026-10-11

```sh
cd ~/server-projects && gh repo clone amitchell-thomson/kairos && cd kairos
cargo install --locked --path crates/kairosd && cargo install --locked --path crates/kairos-cli
python3 -m venv ~/.local/share/kairos/venv && ~/.local/share/kairos/venv/bin/pip install vdirsyncer
ln -sf ~/.local/share/kairos/venv/bin/vdirsyncer ~/.local/bin/vdirsyncer
# config: deploy/chezmoi/dot_config/kairos/config.toml.tmpl (linux branch) → ~/.config/kairos/config.toml
#         deploy/chezmoi/dot_config/vdirsyncer/config.tmpl (linux branch) → ~/.config/vdirsyncer/config
#         config/constraints.example.toml → ~/.config/kairos/constraints.toml
# secrets: ~/.config/kairos/secrets/{api-token,ntfy-topic} generated (chmod 600)
cp deploy/chezmoi/dot_config/systemd/user/kairosd.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now kairosd   # lingering is already on
```

iCloud secrets `~/.config/kairos/secrets/{apple-id,icloud-app-password}` (docs/migration.md §5) are in place; to re-check:

```sh
cd ~/server-projects/kairos && scripts/verify-icloud.sh     # discover, sync, Family check, X-KAIROS round-trip
systemctl --user restart kairosd
journalctl --user -u kairosd -f                              # structured logs
```

Updating: `git pull && cargo install --locked --path crates/kairosd && cargo install --locked --path crates/kairos-cli && systemctl --user restart kairosd`.

## Mac

```sh
cd ~/local-projects/kairos
cargo install --locked --path crates/kairos-cli        # → ~/.cargo/bin/kairos
brew install vdirsyncer                                 # or: uv tool install vdirsyncer
scripts/install-chezmoi.sh && chezmoi diff && chezmoi apply   # config, vdirsyncer (read-only), launchd agent
# Keychain entries: docs/migration.md §5 (kairos-icloud, kairos-apple-id, kairos-api-token, kairos-ics-*)
yes | vdirsyncer discover icloud && vdirsyncer sync && vdirsyncer metasync
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.kairos.sync.plist
kairos sync && kairos doctor
```
Done on Alec's Mac 2026-10-11, except chezmoi itself: the launchd agent and config were rendered from the templates by hand (`chezmoi execute-template < …tmpl` reproduces `~/.config/kairos/config.toml`). The tmux segment (`deploy/tmux.conf.snippet`) and shell summary (`deploy/zshrc.snippet`) are optional and not applied.

## Going live (done 2026-10-11)

1. `scripts/verify-icloud.sh` passes, including `--after-phone-edit`; record results in CLAUDE.md.
2. Try a few writes from the CLI and Claude while `live = false` — they appear in **ZZ-kairos-test** on the phone.
3. Set `live = true` in the server config (`~/.config/kairos/config.toml`, or the chezmoi template) and `systemctl --user restart kairosd`. (`kairosd --live` does the same for one run.)
4. Once live, the `test` source can go (ZZ-kairos-test was deleted on iCloud); keep it only while `live = false`.

## Claude Code

`docs/claude-usage.md` (one `claude mcp add` command). The repo skill is `.claude/skills/kairos/SKILL.md`.

## Phone

The phone shows the synced iCloud calendars and takes events added there; nothing else (plan B.4).
The ntfy pushes, their Approve/Reject buttons and the check-in page were removed on 2026-10-25:
changes are approved in locus's Approve, check-ins happen in the calendar. The ntfy app can be
unsubscribed.

- Shortcut (adds a task over Tailscale): `docs/shortcut.md`.

## Security notes

- kairosd binds the Tailscale IP only; everything except `/healthz` and HMAC-signed `/a/…` and `/checkin…` links needs the bearer token.
- Signed links only approve/reject the one changeset they were made for (HMAC over `id:action`); GET only shows a confirmation page, so link previews can't approve anything.
- Secrets live in the macOS Keychain or `~/.config/kairos/secrets` (0600), never in the repo; secret feed URLs are redacted from errors.
- Phase 9 (public MCP for claude.ai) is deliberately not enabled: it needs proper OAuth.
