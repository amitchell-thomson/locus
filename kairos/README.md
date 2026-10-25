# kairos

A terminal calendar and task scheduler on top of iCloud. iCloud stays the source of truth (iPhone, Siri and notifications keep working); kairos adds a fast ratatui TUI with the locus areas as its calendars, de-duplication, a scheduler that turns tasks into calendar blocks, and a risk-gated write path that Claude can use through MCP.

```
kairos            # full-screen calendar (same as `kairos tui`)
kairos today | week | ls --json | next --tmux
kairos add "Squash" tue 19:00 --key Sport
kairos task add "B4 sheet" --key Engineering --est 4h --due "fri 17:00"
kairos plan | changes --pending | approve <id> | undo <id> | doctor
```

- What it does: `SPEC.md` · architecture & invariants: `CLAUDE.md`
- Setup: `docs/migration.md` → `docs/deploy.md` · Claude: `docs/claude-usage.md` · TUI keys & legend: `docs/tui.md` · phone: `docs/shortcut.md` · JSON: `docs/json.md`
- Tests: `scripts/ci.sh` at the repository root
