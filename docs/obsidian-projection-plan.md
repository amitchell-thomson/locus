# Obsidian projection — implementation plan (CLAUDE.md §13)

> Status: **planning** (2026-06-09). This is the design for the deferred §13 read-only
> visualization layer. Nothing here changes the ingest/retrieval spine.

## 1. What it is (and is not)

A **one-way, regenerable render of the SQLite corpus as an Obsidian vault** — a graph you can
*browse*, never a data source. It exists so the owner can see the corpus's shape (clusters,
bridges, orphans) that query-driven retrieval doesn't expose. It is **not** in the
ingest/retrieval path and retrieval never reads it.

Hard invariants (from §13, restated as acceptance criteria):

1. **One-way.** Export writes `vault/obsidian/`; it is never read back. `vault/obsidian/` is
   gitignored and fully regenerable (`rm -rf` + re-export == identity).
2. **Joins-only.** Every node and edge comes from an existing table via a deterministic
   query. No LLM calls, no inference, no new derived state. (Re-uses `link/related.py`.)
3. **Canonical entities only.** Entity nodes join through `entity_aliases` — never raw
   `entities.name` surfaces. Singletons map to self, so the join is total.
4. **Owns only its subtrees.** The exporter writes only the note subtrees it created and
   **never touches `.obsidian/`** (the user's layout/plugin config). It must be safe to run
   over a vault the user has themselves opened and themed.
5. **`vault/notes/` is an input, not an output.** Authored notes are ingested like any other
   source; the export is a separate output tree.

## 2. Node & edge model

| Obsidian object | Source | Notes |
|---|---|---|
| **Doc note** | one per `documents` row | frontmatter = synthesis (`thesis/method/result/limitations`) + `category`, `source_type`, `source_date`, `title`, `source_uri`. Body = section headings + per-section summary. |
| **Canonical entity note** | one per distinct `(canonical_name, canonical_type)` in `entity_aliases` that spans ≥2 docs | body lists the variant surfaces it subsumes + backlinks to the docs that mention it. Single-doc canonicals are **not** emitted (noise; mirrors the cross-doc-canonical payload in `related.py`). |
| Section | `## ` heading inside its doc note | NOT a standalone note (§13). |
| Chunk / proposition | — | NOT nodes (§13). |
| **Edge: doc → entity** (`mention`) | `entities ⨝ entity_aliases` | wikilink from doc note to each canonical entity note it mentions. |
| **Edge: doc ↔ doc** (`related`) | `related_documents()` top-N | "Related" section of wikilinks, reusing the IDF-weighted, stop-entity-guarded ranking already shipped. Identical ranking to `locus inspect`. |
| **Edge: entity → doc** (backlink) | inverse of mention | Obsidian derives backlinks automatically from the doc→entity links; no extra writing. |

Co-occurrence edges (entity↔entity) are deferred to a later phase — they explode quadratically
and Obsidian's graph already shows them transitively through shared doc nodes.

## 3. File layout

```
vault/obsidian/                      # exporter-owned root (gitignored)
├── docs/
│   └── <category>/<slug>.md         # one per document; slug from title, id-suffixed on collision
├── entities/
│   └── <type>/<canonical-slug>.md   # one per cross-doc canonical
└── _index.md                        # generated TOC: counts, orphans, biggest clusters
# .obsidian/  ← NEVER written or deleted by the exporter
```

- **Slugs** must be stable across re-exports (so Obsidian's own graph positions/aliases
  survive) and filesystem-safe. Strategy: slugify(title), disambiguate collisions with the
  numeric doc id (`title-(217)`), which is already stable per §retitle. Entity slugs:
  slugify(canonical_name) + numeric cluster_id on collision.
- **Wikilinks** use Obsidian's `[[path|alias]]` so the link target is the stable slug while
  the visible text is the human title.

## 4. Module & CLI

```
locus/export/
├── __init__.py
└── obsidian.py        # export_vault(conn, out_dir, *, top_related, include_excluded) -> ExportReport
```

- Pure functions: `doc_note_markdown(doc_row, sections, related, entities) -> str`,
  `entity_note_markdown(canonical, variants, mentioning_docs) -> str`, `slug(...)`. These are
  string-in/string-out and unit-testable with seeded rows (model-free, per §14).
- `export_vault` orchestrates: query → render → write. **Mirror-write discipline** (like
  `backup._copy_tree` + manifest): write into a temp staging dir, then atomically swap, and
  delete only files under `docs/` and `entities/` that the current run did not emit — so a
  deleted document's stale note is removed without ever touching `.obsidian/` or `_index.md`
  history. Track emitted paths in an `ExportReport`.
- **CLI**: `locus export-obsidian [--dest vault/obsidian] [--top-related N] [--include-excluded]`.
  Respects `[retrieve].exclude_source_uris` by default (the self-ingested locus repo stays out
  of the projection too), with the same override flag the retrieval CLI uses.
- **Manual-only**, like `link`/`retitle` — it reads the alias substrate, so it should run
  *after* `locus link`. Emit a warning (reuse the `status` freshness check) if
  `alias_uncovered_surfaces > 0` so the user knows the projection predates the latest ingest.

## 5. Idempotency & safety

- Re-export is deterministic given the same DB: stable slugs + sorted iteration ⇒ no churn,
  clean git/Obsidian diffs.
- The exporter computes its owned path set first and only ever unlinks within `docs/` and
  `entities/`. A guard asserts `out_dir` is non-empty and not a parent of the DB before any
  delete (belt-and-braces against a misconfigured `--dest`).
- `.obsidian/` carve-out already in `.gitignore` (`!vault/obsidian/.obsidian/`).

## 6. Config

Add an optional `[obsidian]` section (defaults clean, per §14):

```toml
[obsidian]
out_dir = "vault/obsidian"     # exporter-owned root
top_related = 5                # doc↔doc edges per note (matches related_documents default)
min_cluster_docs = 2           # canonical entity emitted only if it spans ≥ this many docs
emit_entity_notes = true       # entity notes can be turned off for a docs-only graph
```

## 7. Tests (model-free)

- `slug()` stability + collision disambiguation.
- `doc_note_markdown` renders synthesis frontmatter + section headings from seeded rows.
- `entity_note_markdown` joins through a seeded `entity_aliases` (asserts canonical, not raw).
- `export_vault` over a seeded 3-doc DB: correct files created; a second run after deleting a
  doc removes that note and **leaves a sentinel file under `.obsidian/` untouched**; re-export
  is byte-identical (idempotency).
- Guard test: refuses to delete outside its owned subtrees.

## 8. Phasing

1. **Phase 1 — docs-only graph.** Doc notes + `related` edges + `_index.md`. Immediately
   useful, exercises the mirror-write/ownership discipline. No entity notes yet.
2. **Phase 2 — canonical entity notes.** Add `entities/` and doc→entity mention links; the
   graph view gains the entity hubs.
3. **Phase 3 (optional) — richer views.** Entity↔entity co-occurrence, category dashboards,
   Dataview-friendly frontmatter. Only if the graph proves useful enough to live in.

## 10. Viewing from the Mac (transport)

The export runs on the home server; Obsidian is a GUI that runs on the Mac. The data and the
UI are on opposite sides of the SSH boundary, so the projection must be *transported* to the
Mac to be viewed. **Recommended: rsync-pull to a Mac-local vault.**

```bash
# on the server (after `locus link`):
ssh server "cd ~/server-projects/locus && uv run locus export-obsidian"

# pull to the Mac (run from the Mac):
rsync -az --delete --exclude '.obsidian/' \
  server:~/server-projects/locus/vault/obsidian/ \
  ~/LocusVault/
# then in Obsidian: Open folder as vault → ~/LocusVault
```

Why pull (vs. an SSHFS mount): Obsidian's graph view, file watcher, and indexing read a local
SSD instead of round-tripping every stat over SSH — usable vs. sluggish at ~2,500 notes.

The `--delete --exclude '.obsidian/'` flags are the transport-layer restatement of invariant
**#4**: stale doc notes are removed on the Mac, but the Mac's `.obsidian/` (layout, theme,
plugins, graph positions) is never clobbered. The **Mac owns its `.obsidian/`** — that config
is per-machine and lives only on the Mac, which keeps the projection strictly one-way
(invariant #1): the vault is a render target you browse, never edit. SSHFS is the zero-copy
alternative (re-export is instantly visible) at the cost of GUI lag and putting `.obsidian/`
inside the exporter-owned tree on the server; acceptable but not preferred.

## 9. Open questions (resolve before Phase 1)

- **Figures**: link the raw-store PNG into the doc note as an embedded image (`![[...]]`)?
  Obsidian renders local images; the PNGs live in `vault/raw/{hash}_fig{N}.png`. Leaning yes,
  read-only reference (no copy) — but that points a vault file outside `vault/obsidian/`, so
  may need a symlink or a copied thumbnail. Decide in Phase 2.
- **Scale**: 291 docs + ~2,200 cross-doc canonicals ≈ 2,500 notes — fine for Obsidian. Re-check
  if the corpus 10×'s.
- **Slug churn vs. retitle**: titles change when `locus retitle` reruns; id-suffixed slugs
  keep filenames stable, but the visible alias updates — acceptable (the graph identity is the
  file, not the title).
