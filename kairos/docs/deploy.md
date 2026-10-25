# Deploying kairos

kairos is built and deployed from the locus repository like the rest of locus (ADR-008, L8).

```
iPhone / Calendar.app ─┐
                 iCloud (truth)
        ┌───────────┴────────────┐
 vdirsyncer rw (compute-node)   rsync of the server's copy (Mac, launchd every 60 s)
        │                        │
 kairosd :7420 (Tailscale)  ◄── kairos CLI / TUI writes (HTTP + bearer)
        ▲  MCP /mcp  ◄── Claude Code
        ▲  /api/…    ◄── the locus window (tasks, chores, Approve), the iOS Shortcut
```

## Where things are

| What | Where |
|---|---|
| The server's unit | `deploy/systemd/locus-kairosd.service`, enabled in `deploy/systemd/enabled` |
| Getting code to the server | `deploy/deploy.sh` from the laptop: push, pull on the server, then `deploy/server-sync.sh` runs `cargo install --locked` for kairosd and kairos when Rust sources moved, restarts the unit and runs `kairos doctor` |
| The Mac's config, launchd agent, vdirsyncer config | `deploy/kairos/chezmoi/` (templates; rendered by hand on the Mac, chezmoi itself not applied) |
| Optional tmux segment and shell summary | `deploy/kairos/tmux.conf.snippet`, `deploy/kairos/zshrc.snippet` (not applied) |
| Setup scripts | `deploy/kairos/install-chezmoi.sh`, `store-secrets.sh`, `verify-icloud.sh` |
| The CI gate | `scripts/ci.sh` at the repository root |

On the Mac: `cargo install --locked --path kairos/crates/kairos-cli` from the locus checkout puts
`kairos` on your PATH; the locus window's calendar runs it.

## Setup, as it was done (2026-10-11)

Secrets live in the macOS Keychain or `~/.config/kairos/secrets` (0600), never in the repository:
the API token, the iCloud app password and Apple ID (`docs/migration.md` §5). vdirsyncer runs
read-write on the server only. `deploy/kairos/verify-icloud.sh` checks discovery, sync, the
Family calendar and that `X-KAIROS-*` properties survive a round trip and a phone edit; its
results are in `kairos/CLAUDE.md`. kairos has been live (`live = true`) since 2026-10-11.

## Claude Code

`docs/claude-usage.md` (one `claude mcp add` command). The skill is `kairos/.claude/skills/kairos/`.

## Phone

The phone shows the synced iCloud calendars and takes events added there; nothing else (plan B.4).
The ntfy pushes, their approve buttons and the check-in page were removed on 2026-10-25: changes
are approved in locus's Approve, check-ins happen in the calendar.

- Shortcut (adds a task over Tailscale): `docs/shortcut.md`.

## Security notes

- kairosd binds the Tailscale IP only; everything except `/healthz` needs the bearer token.
- Secret feed URLs are redacted from errors.
- A public MCP for claude.ai is deliberately not enabled: it needs proper OAuth.
