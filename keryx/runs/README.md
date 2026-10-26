# Run prompts

`keryx run morning|evening` and `keryx cleanup` fill these in and hand them to headless Claude Code (`claude -p`) with only keryx's MCP tools (SPEC §8.5). `evening.md` is added to `morning.md` at `{{extra}}`. Placeholders: `{{run_id}}`, `{{now}}`, `{{since}}`, `{{max_threads}}`, `{{accounts}}`, `{{instructions}}`. `keryx run morning --dry-run` prints the result.
