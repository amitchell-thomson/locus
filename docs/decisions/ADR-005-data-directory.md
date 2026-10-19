# ADR-005 — The data directory is `data/`, not `vault/`

**Status:** accepted
**Date:** 2026-10-19
**Your words:** "the vault should instead be called library or something better as I do not want it being confused with obsidian's vault"; then, of `library/` or `data/`, "data/, and do not worry at all about any running MCP sessions" (19 Oct)

## Context
`vault/` in the checkout holds the database, the raw store, the drop zone (`incoming/`), his
notes, backups, the `.rmdoc` cache, logs and the Obsidian export (`vault/obsidian/`). The name
reads as an Obsidian vault, and one sits inside it. `library/` was considered and passed over:
the plan uses "the library" for the frozen engine and its sources, and this directory also holds
his notes, backups and caches.

Paths under the directory are stored as identity, not only as locations: `documents.source_uri`
is how an edited note is found and replaced, and the same strings key the agent tables. Renaming
the directory alone would have made every edited note arrive as a second document, silently.

## Decision
Rename the directory to `data/` everywhere: config defaults and `config.example.toml`, code,
tests, units, the outbox, CLAUDE.md. Migration 0037 rewrites the stored paths (1,549 rows across
14 columns on the server, trialled on a copy before deploying). The server moves its directory
once, inside `deploy/server-sync.sh`, under the ingest flock, and rewrites its `config.toml`.
No compatibility symlink.

## Consequences
- `locus mcp` processes started before the deploy hold the old config and fail to open the
  database until their session restarts (accepted: "do not worry at all about any running MCP
  sessions").
- Corpus text that mentions `vault/` (the ingested Locus repository and docs about it) still
  says so: it is what those documents said, and derived layers do not edit the corpus.
- Model prompts that call the corpus a "knowledge vault" are unchanged, so pass caches keyed on
  them stay valid. User-facing wording (CLI help, the MCP descriptions) now says "library".
- The `locus/vault/` package (the notes writer) keeps its name; it is code, not the directory.
- Docs written before the rename (`docs/*.md` outside the redesign) keep the old paths as history.

## What would make this wrong
Nothing foreseeable; it is a name. If the directory ever splits (library store vs his notes),
name the parts then.
