# The Home chat's files

Read at every turn by `src/chat.rs`, so an edit applies to the next message without a rebuild.

- `brief.md`: the system prompt appended to Claude Code's own: who it is talking to, what it is
  for, where things are, and its rules.
- `settings.json`: its permission rules (files-and-chat M2 step 4). `__HOME__` is filled in at
  each turn. The chat runs under `--permission-mode dontAsk`, so anything not allowed here is
  refused, never asked: keryx's archive, file, snooze and cleanup tools are left out on purpose
  (read and draft only), and edits outside `~/files` and the registry go through `o`, full
  Claude Code, where you approve them as usual.

Path rules use the glob form, `Bash(mkdir ~/files/*)`: the prefix form `Bash(mkdir ~/files/:*)`
matches whole words only, so it never matched `mkdir ~/files/Personal/x` (measured 9 Nov: the
chat's first folder was refused under it). A move must name `~/files` on both sides.
