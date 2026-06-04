# CLAUDE.md — Locus

> Drop-in context for Claude Code. This file is authoritative for architecture and
> conventions. Where it conflicts with ad-hoc instructions in chat, ask before deviating.

---

## 1. What Locus is

Locus is a **self-hosted system for querying, linking, and serving as Claude's context over
the owner's entire personal knowledge base** — not just technical reading, but the full record
of what the owner has learned, built, and achieved: papers, code, lecture/seminar notes,
videos, **and personal/professional material — projects, achievements, write-ups, historical
records, and general information.** It ingests that heterogeneous corpus and supports intelligent
retrieval over it via a CLI.

**Three primary uses (all equal):**
1. **Query** — ask questions and get grounded, cited answers over the whole corpus.
2. **Link** — surface connections *across* the corpus (cross-domain, cross-project, cross-time),
   tying disparate material together.
3. **Give Claude context** — act as a retrieval/memory backend that feeds the owner's own
   knowledge into Claude on demand (today via the CLI/`query`; a context-provider surface —
   e.g. an MCP server or retrieval API — is a natural future surface, see §15).

Engineering ↔ quantitative-finance synthesis, gap analysis, and professional framing are
*important use cases within this*, not the whole scope. The corpus is broad and personal.

**Workflow is query-driven, not browse-driven.** There is no bespoke UI in phase 1 (an Obsidian
projection, §14, is an optional visual layer *on top of* the data, never the primary surface).

**Design objective, stated precisely:** maximise *retrieval answer quality* per query.
Ingest cost (time, compute) is treated as effectively unbounded; **retrieval latency and
answer quality are the only constraints that matter.**

---

## 2. Core principles (these govern every design decision)

1. **Local data ownership.** Everything lives on the local server. No cloud storage of
   corpus content. The only network egress at runtime is the Claude API call for final
   generation.
2. **Terminal-first.** CLI is the product surface. No GUI until the core pipeline is proven.
3. **Pragmatic build-vs-buy.** This is a custom build because off-the-shelf tools
   (AnythingLLM, Open WebUI, Khoj, NotebookLM, Obsidian plugins) were evaluated and rejected
   *as the retrieval/knowledge engine* — none does hierarchical L1/L2/L3 + rerank retrieval.
   Do not reintroduce a dependency that re-buys that engine. **Narrow exception:** Obsidian is
   permitted strictly as a *read-only projection / visualization layer* rendered over the
   SQLite source of truth (see §14). In that role it is a rendering target, not a dependency in
   the retrieval path, and does not re-buy what was rejected.
4. **Quality over speed at ingest.** Ingest passes have no time budget. Never trade ingest
   quality for throughput.
5. **Local models for ingest, Claude API for generation.** Forced by the 8GB VRAM ceiling.
   *Caveat — see §11 open decisions: the proposition/synthesis passes are generation-quality
   work running on the weakest model in the stack, so this split is provisional for those
   passes specifically, not settled.*
6. **The three-level schema generalises across all source types.** PDFs, code, and video all
   reduce to the same L1/L2/L3 structure so retrieval logic stays unified. Do not fork the
   retrieval path per source type.
7. **Idempotent ingest by content hash.** Re-ingesting the same content is a no-op.

---

## 3. Hardware envelope (the binding constraints)

| Resource | Spec | Consequence |
|---|---|---|
| GPU | RTX 3070 Ti, **8 GB VRAM** | **Binding constraint.** Local LLMs limited to ~8B params, quantised. |
| CPU | Ryzen 5 5600X | Reranker runs here (cross-encoder, CPU). |
| RAM | 32 GB | Comfortable for SQLite + Ollama host overhead. |
| Storage | 1 TB SSD | Flat raw-file store + SQLite DB. |

The 8 GB VRAM ceiling is *the* architectural driver: it is why ingest is local and
generation is API. Any proposal that assumes a larger local model is out of scope unless the
hardware changes.

---

## 4. Tech stack

| Layer | Tool | Notes |
|---|---|---|
| PDF extraction | `pymupdf` | text + structure; preserve raw LaTeX for math-heavy sections |
| Code parsing | `python-ast` | per-file functions + call graph |
| Video transcripts | `youtube-transcript-api` | timestamps retained for `?t=` deep links |
| Embeddings | `nomic-embed-text` via Ollama | **768-dim** — fixed across the vector tables |
| Local LLM (ingest) | `llama3.1:8b` **or** `qwen2.5:7b` via Ollama | benchmark both; see §11 |
| Vector store | `sqlite-vec` | brute-force KNN; acceptable at personal-corpus scale (see §11) |
| Metadata + schema | SQLite | single DB file |
| Reranker | `ms-marco-MiniLM` cross-encoder | runs on **CPU** |
| RAG generation | Claude API | **single call per query** |
| (Existing infra available) | Parquet, PostgreSQL, DuckDB | not required for phase 1 |

---

## 5. Data model

Three levels per document. Same shape for every source type. Committed DDL below is
authoritative for phase 1. (`db/schema.sql` is a maintained human-reference snapshot;
the operational source of truth is the Alembic migration set — see below.)

```sql
-- Migration tracking is owned by Alembic (the `alembic_version` table, created
-- automatically). Schema is managed forward-only via Alembic migrations in
-- db/migrations/versions/, applied with `alembic upgrade head`. REQUIRED from day 1:
-- re-running full ingest is the single most expensive operation, so never force a
-- re-ingest just to change schema. Migrate forward instead.
-- (Note: an earlier draft used a hand-rolled `schema_version` table; Alembic replaces it.)

-- L1 — document
CREATE TABLE documents (
    id           INTEGER PRIMARY KEY,
    content_hash TEXT    NOT NULL UNIQUE,          -- idempotency key
    source_type  TEXT    NOT NULL CHECK (source_type IN ('pdf','code','video')),
    source_uri   TEXT    NOT NULL,                 -- original path or URL
    raw_path     TEXT    NOT NULL,                 -- location in flat raw store
    title        TEXT,
    ingested_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    ingest_model TEXT    NOT NULL,                 -- local LLM that produced synthesis
    -- doc synthesis (kept as discrete columns, not a JSON blob, so they're queryable)
    thesis       TEXT,
    method       TEXT,
    result       TEXT,
    limitations  TEXT,
    section_map  TEXT,                             -- JSON: ordered section index
    gap_flags    TEXT                              -- JSON: array of flagged gaps
);

-- L2 — section
CREATE TABLE sections (
    id                 INTEGER PRIMARY KEY,
    doc_id             INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    position           INTEGER NOT NULL,           -- order within doc
    title              TEXT,
    summary            TEXT,                        -- LLM section summary
    -- propositions are first-class rows in the `propositions` table below (decision A, RESOLVED),
    -- NOT a JSON blob here, so they can be embedded and directly retrieved. Single source of truth.
    cross_section_deps TEXT,                        -- JSON: section ids this depends on
    -- code-specific
    file_path          TEXT,                        -- per-file for code
    call_graph         TEXT,                        -- JSON for code
    UNIQUE (doc_id, position)
);

CREATE VIRTUAL TABLE section_vectors USING vec0(
    section_id INTEGER PRIMARY KEY,
    embedding  FLOAT[768]
);

-- L3 — chunk
CREATE TABLE chunks (
    id              INTEGER PRIMARY KEY,
    section_id      INTEGER NOT NULL REFERENCES sections(id)  ON DELETE CASCADE,
    doc_id          INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL,
    raw_text        TEXT    NOT NULL,               -- ~512 tokens
    embed_model     TEXT    NOT NULL,               -- model that produced the vector
    -- provenance
    file_path       TEXT,                           -- code
    line_start      INTEGER,                        -- code
    line_end        INTEGER,                        -- code
    video_timestamp INTEGER,                         -- video: seconds, for ?t=
    UNIQUE (section_id, position)
);

CREATE VIRTUAL TABLE chunk_vectors USING vec0(
    chunk_id  INTEGER PRIMARY KEY,
    embedding FLOAT[768]
);

-- entities: typed + section-anchored provenance
CREATE TABLE entities (
    id         INTEGER PRIMARY KEY,
    doc_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    section_id INTEGER          REFERENCES sections(id)  ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    type       TEXT    NOT NULL,                     -- method|dataset|author|concept|ticker|...
    UNIQUE (doc_id, section_id, name, type)
);
CREATE INDEX idx_entities_name ON entities(name);

-- tags: doc-level, normalised
CREATE TABLE tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE TABLE doc_tags (
    doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id)      ON DELETE CASCADE,
    PRIMARY KEY (doc_id, tag_id)
);
```

### Propositions as first-class retrieval units (decision A — RESOLVED, committed)

Propositions are the highest-signal knowledge unit, so they are a **direct retrieval target**,
not JSON hidden on a section. They get their own table + their own `vec0` embeddings and are
searched alongside chunks (L3) and section summaries (L2). This is committed schema, written
in the same per-document transaction as the other levels.

```sql
-- propositions: atomic, self-contained claims, embedded + directly retrievable
CREATE TABLE propositions (
    id          INTEGER PRIMARY KEY,
    section_id  INTEGER NOT NULL REFERENCES sections(id)  ON DELETE CASCADE,
    doc_id      INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,                  -- order within the section
    text        TEXT    NOT NULL,                  -- the claim, self-contained
    embed_model TEXT    NOT NULL,                  -- model that produced the vector
    UNIQUE (section_id, position)
);

CREATE VIRTUAL TABLE proposition_vectors USING vec0(
    proposition_id INTEGER PRIMARY KEY,
    embedding      FLOAT[768]
);
```

**Why first-class (the rationale, kept for posterity):** if propositions only lived as JSON on
the section they would never be embedded, so the vector search could never match one directly —
they would surface only via hierarchical expansion. Promoting them to embedded rows makes the
single highest-value unit searchable. Embedding is the expensive, irreversible step, so this was
decided **before** the first full ingest to avoid a later re-embed.

---

## 6. Ingest pipeline

Orchestrated in `ingest_pipeline.py`. Idempotent by `content_hash`. Writes all three levels
in a single transaction per document.

```
source file
  → hash; if hash exists in documents → skip (no-op)
  → copy raw file into flat raw store (raw_path)
  → EXTRACT (source-specific):
        pdf   : pymupdf → text + section structure; preserve LaTeX in math sections
        code  : python-ast → functions, per-file structure, call graph
        video : youtube-transcript-api → transcript + timestamps
  → INGEST PASSES (local LLM, quality-only, unbounded time):
        section pass : summary + propositions + quant results + cross-section deps
        doc synthesis: thesis + method + result + limitations
        entity pass  : typed entities + section provenance
        gap flagging : doc-level gap_flags
        (code)       : function extraction + repo synthesis
        (video)      : transcript cleaning + structure injection, THEN standard passes
  → EMBED (nomic-embed-text): L3 chunks (512 tok) + L2 section summaries + propositions
  → WRITE L1/L2/L3 + entities + tags in one transaction
```

**Hard rule for every structured LLM output (propositions, entities, synthesis, gaps):**
validate against a schema (pydantic or equivalent), with **bounded retries and a repair
prompt** on malformed JSON. An 8B quantised model *will* emit invalid/partial JSON; the
pipeline must never silently write garbage or crash a batch on one bad doc. Log and quarantine
failures; do not abort the run.

---

## 7. Retrieval pipeline

In `retrieve/` + `query.py`. Single Claude API call at the end.

```
embed query (nomic)
  ├─ proposition search: top 10 over proposition_vectors (highest-signal claims)
  ├─ fine search   : top 20 over chunk_vectors      (L3)
  ├─ section search: top 5  over section_vectors    (L2)
  └─ entity-anchored search: only for named-entity queries (exact/normalised match on entities.name)
merge candidates
  → rerank top 8 with ms-marco-MiniLM cross-encoder (CPU)
  → hierarchical expansion: for each survivor, fetch parent section summary + doc synthesis
                            via plain SQLite joins (free, no inference)
  → context assembly, coarse-to-fine: doc summaries → section summaries → chunks
                            (enforce a token budget cap; see config)
  → Claude API: 1 call
```

- **Code results** must include the function body + `file_path:line_start-line_end`.
- **Video results** must include the `?t=<seconds>` deep-link URL.
- **Query modes** (standard RAG, gap analysis, cross-domain synthesis, code recall,
  professional framing, project recommendation) are a **system-prompt lever only** — they do
  **not** change the retrieval pipeline. Implement them as swappable system prompts over the
  same assembled context.
- **Concept graph / coverage map** are generated from SQLite metadata with **no inference**
  (joins over entities/tags/sections). Useful for navigation absent a UI. Cheap; build when
  convenient.

---

## 8. Repository layout (target)

```
locus/
├── CLAUDE.md
├── pyproject.toml              # prefer uv; pip works
├── alembic.ini                 # Alembic config; DB url resolved at runtime from config.toml
├── config.toml                 # see §9
├── locus/
│   ├── config.py               # load + validate config.toml
│   ├── db/
│   │   ├── schema.sql           # human-reference snapshot (NOT applied directly)
│   │   ├── connection.py        # opens SQLite, loads sqlite-vec extension
│   │   ├── migrate.py           # in-process Alembic driver: migrate()/current_revision()
│   │   └── migrations/          # Alembic env.py + script.py.mako + versions/ (forward-only)
│   ├── extract/  { pdf.py, code.py, video.py }
│   ├── ingest/   { sectioner.py, summarize.py, propositions.py, entities.py,
│   │               synthesis.py, gaps.py, embed.py }
│   ├── ingest_pipeline.py      # orchestration + idempotency + transactional write
│   ├── retrieve/ { search.py, rerank.py, expand.py, assemble.py }
│   ├── query.py                # retrieve → assemble → Claude API
│   ├── watcher.py              # /vault/incoming/ watch + source-type routing
│   └── cli.py                  # entrypoint (the product surface)
├── tests/
└── vault/
    ├── incoming/               # watcher source; auto-routed by type
    ├── raw/                    # flat raw-file store
    └── notes/                  # owner's manual annotations (watched; ingested as docs)
```

---

## 9. Environment & config conventions

- **Python 3.11+**, managed with **uv** (`uv venv`, `uv pip install`). Type hints mandatory.
- **No secrets in code or config committed to git.** Claude API key via env var
  (`ANTHROPIC_API_KEY`); `config.toml` may be templated as `config.example.toml`.
- All tunables live in `config.toml`, not scattered constants. Minimum set:

```toml
[ollama]
host          = "http://localhost:11434"
embed_model   = "nomic-embed-text"
ingest_model  = "llama3.1:8b"     # or "qwen2.5:7b" — benchmark decides

[paths]
db            = "vault/locus.db"
raw_store     = "vault/raw"
incoming      = "vault/incoming"
notes         = "vault/notes"

[embed]
dim           = 768               # locked to nomic-embed-text; changing => full re-embed
chunk_tokens  = 512

[retrieve]
proposition_top_k = 10            # highest-signal claims, searched as first-class units
fine_top_k    = 20
section_top_k = 5
rerank_top_k  = 8
context_token_budget = 100000     # cap on assembled context to the Claude API call

[generation]
model         = "claude-..."      # set to current model id
```

---

## 10. Build order

Build in **working vertical slices** — prove the core retrieval path end-to-end on one
source type before adding breadth. Defer UI entirely.

**Phase 1:**
1. Ingest script for **PDFs** with the full 3-level schema (extract → passes → embed → write).
2. **RAG CLI** with hierarchical retrieval (the full §7 pipeline) over the PDF corpus.
3. **Folder watcher** at `vault/incoming/`, auto-routing by source type.
4. Full **Llama ingest passes** (section / synthesis / entity / gap), quality-tuned.
5. **Code repo** ingest (python-ast, call graph, per-file sections).
6. **YouTube** ingest (transcript clean + structure injection → standard passes).

Slices 1→2 are the critical path: a single PDF ingested correctly and a query answered
end-to-end is the first milestone. Everything else is breadth on a proven spine.

**Hardening (see §15):** after the Stage 6/7 retrieval spine is proven and *before* the
multi-year bulk ingest, do **Stage 5.1** (math-aware extraction, figures, scanned-PDF OCR,
entity resolution — all re-ingest-bound). Hybrid lexical retrieval folds into Stage 6.

**Corpus scope (broad, personal — see §1):** technical reading (quant-finance papers,
control-systems / signal-processing notes, seminar notes, code) **plus** personal/professional
material — project write-ups, achievements, CV/portfolio material, historical records, and
general notes. This heterogeneity (many formats, not just PDF; content with its own dates and
categories) drives several pre-bulk-ingest decisions — see §15.

---

## 11. Open decisions & known risks (resolve deliberately, don't drift past them)

**A. Propositions as a retrieval unit — RESOLVED.** Propositions are promoted to a first-class,
embedded, directly-retrievable table (committed DDL in §5; embedded in §6; searched in §7 with
`proposition_top_k`, §9). Chosen over downgrading them to context-only material because they are
the highest-signal unit. Decided before the first full ingest, so no re-embed is needed.

**B. The weakest model owns the highest-value pass.** Proposition/synthesis extraction is
generation-quality work on an 8B quantised model; quality will be inconsistent. Mitigations:
(i) strict schema-validation + bounded repair retries (already mandated, §6); (ii) treat
"local vs Claude API for the proposition pass specifically" as an *empirical* question —
measure proposition quality from the 8B model on a sample before committing to local-only.
The "local for ingest" principle should not pre-empt that measurement.

**C. Local-model benchmark unresolved.** `llama3.1:8b` vs `qwen2.5:7b` for ingest quality —
decide empirically on a fixed sample (proposition discreteness/faithfulness, entity recall,
synthesis accuracy), not by reputation. Build a tiny eval harness; this is cheap insurance
given ingest is the expensive, hard-to-redo step.

**D. sqlite-vec does brute-force KNN (no ANN index).** Fine at personal-corpus scale (linear
scan over even ~10⁵ vectors is sub-second). Add a candidate-count check that warns if the
vector count grows into a range where latency degrades, so the constraint surfaces before it
bites rather than after.

**E. "Parallel" fine + section search.** SQLite + the GIL make this logically-concurrent at
best. Treat "parallel" as "both queries issued before merge," not as a threading requirement.
Don't over-engineer concurrency here.

**F. Content-hash idempotency is whitespace-sensitive.** A re-downloaded or re-exported
source with trivial formatting differences will re-ingest as a new doc. Acceptable for now;
revisit with content normalisation before hashing if duplicate docs become a problem.

**G. Context-budget enforcement.** Coarse-to-fine assembly can overflow the model window on
large docs. The `context_token_budget` cap (§9) must be enforced in `assemble.py`, truncating
finest-grained (chunk) content first.

---

## 12. Out of scope (phase 1)

- Any *custom-built* GUI / web front end.
- Obsidian *as a retrieval/knowledge engine* (rejected, see §3.3). **Note:** Obsidian as a
  read-only projection layer over SQLite is **not** out of scope — it is deferred to
  post-slice-2 and specified in §14. `vault/notes/` still covers manual annotation and ingests
  as before.
- Cloud storage of corpus content.
- Multi-user / auth.
- Models larger than ~8B locally (hardware-bound).

---

## 13. Coding conventions

- Type-hinted, explicit, small functions. State assumptions in docstrings where non-obvious.
- Structured LLM I/O goes through validated models (pydantic), never raw dict access.
- Every external call (Ollama, Claude API, file IO) has explicit error handling; ingest
  failures quarantine the single doc and continue the batch.
- DB access is transactional per document; schema changes go through **Alembic migrations**
  in `db/migrations/versions/` (applied with `alembic upgrade head`), never ad-hoc `ALTER`
  against a live DB. vec0 virtual tables are created via raw `op.execute(...)` in migrations
  (autogenerate cannot model them); `env.py` loads the sqlite-vec extension on the migration
  connection.
- Prefer correctness and legibility over cleverness. No premature optimisation of ingest;
  optimise retrieval latency only where measured.

---

## 14. Obsidian projection layer (deferred — post-slice-2)

A **read-only visualization/navigation layer** rendered over the SQLite source of truth.
Obsidian is a *render target*, not part of the ingest or retrieval path. This is the
"concept graph / coverage map" navigation described in §7, materialised in Obsidian instead
of a bespoke viewer.

**Non-negotiable invariants:**

1. **SQLite is the single source of truth.** The export is one-way: `SQLite → vault/obsidian/`.
   The generated vault is **never read back** into the DB.
2. **Generated, disposable, regenerable.** `vault/obsidian/` is produced by a pure function of
   the DB and can be deleted and rebuilt at any time. It is **gitignored**.
3. **Authored notes are unaffected.** Hand-written notes still live in `vault/notes/` and ingest
   into SQLite via the watcher exactly as before. They are an *input*; `vault/obsidian/` is an
   *output*. Do not conflate the two directories.
4. **Outside the spine.** Lives in `locus/export/obsidian.py`. Building or breaking it must not
   touch ingest or retrieval. No-inference, joins-only (same constraint as §7's concept graph).

**Sequencing:** built only after Phase-1 slices 1–2 (PDF ingest + RAG CLI) are proven
end-to-end. There is nothing meaningful to visualise until the entity pass has run on real docs.

**Node / edge granularity:**

| Schema level | Becomes | Notes |
|---|---|---|
| Document (L1) | One note file | Anchor node. Frontmatter: `thesis/method/result/limitations/tags/gap_flags`; body renders `section_map` as headings. |
| Entity | One note file | The connective tissue — drives the cross-domain "linked ideas" graph (§1 objective). Identity key: see open sub-decision below. |
| Tag | Native `tags:` frontmatter | No separate files. |
| Section (L2) | Headings within the doc note | Not separate files (keeps graph legible). `cross_section_deps` → links. |
| Chunk / proposition (L3) | Not nodes | Too fine. |

Edges derive from existing data only: doc↔entity (mention), entity↔entity (co-occurrence),
doc↔doc (shared entities / `cross_section_deps`), `gap_flags` as callouts. Structured YAML
frontmatter enables Dataview / Mermaid / Charts dashboards (coverage map = `tag × source_type`
matrix) with no custom code.

**Resolved sub-decisions:**
- **Entity node identity = `name+type`.** Matches the schema's
  `UNIQUE(doc_id, section_id, name, type)` and never wrong-merges distinct entities that share
  a string. Known cost: the 8B model's inconsistent `type` labelling (§11.B/C) can produce
  near-duplicate nodes for one real concept. *Mitigation deferred:* if the real post-slice-2
  graph shows duplicate-fragmentation, add a view-layer alias/normalization map **in the
  exporter only** (never altering the DB). Do not build the alias layer pre-emptively.
- **`vault/obsidian/` placement & git.** Lives at `vault/obsidian/`, sibling to
  `incoming/ raw/ notes/`. **Gitignored** (derived, regenerable). The exporter owns and
  regenerates only its own subtrees (e.g. `docs/`, `entities/`) and **must never touch
  `.obsidian/`** (Obsidian's own plugin/graph config) — regenerate per-owned-subtree, never
  blanket-wipe the folder.

---

## 15. Planned hardening & future work (Stage 5.1 + retrieval), with sequencing

This records the spec for hardening identified while validating ingest quality on a real
corpus. Do not lose it.

### 15.0 The governing principle — what Claude can and cannot recover
This is RAG: **Claude only ever sees what retrieval surfaces.** Recoverability of poor ingest
depends on *where* quality was lost:
- **Generation-time noise** (coarse/slightly-wrong summary, proposition, entity; redundant
  context) → **recoverable**, *because retrieval assembles raw chunks + section summaries + doc
  synthesis, not just the LLM-derived units*, so Claude grounds on source text. Keep this
  property: always retrieve and assemble the raw chunk alongside derived units.
- **Extraction loss** (garbled/lost equations, scanned-no-text, ignored figures) → **not
  recoverable** — the information never entered the system.
- **Retrieval miss** (right content exists but isn't surfaced) → **not recoverable** — Claude
  never sees it.
Prioritise the two Claude *cannot* fix: clean content **in** (math/figures), right content **out**
(hybrid retrieval).

### 15.1 Stage 5.1 — ingest hardening (RE-INGEST-BOUND; settle before scaling the corpus)
These change stored/embedded data, so adding them later forces a full re-ingest. Do them
**after** the retrieval spine (Stage 6/7) is proven but **before** the multi-year bulk ingest.

- **Math-aware extraction.** The text layer garbles or drops equations; the corpus is
  math-dense (quant/control/signal-processing). Route `has_math` regions (already flagged in
  `extract/pdf.py`) through a math model — Nougat or GROBID for papers, or a vision/OCR pass —
  to recover LaTeX. **Not Claude-recoverable; highest-value gap.**
- **Figures / diagrams** (PDFs often contain block diagrams, plots, schematics). Three tiers:
  1. *Preserve* — detect figure regions, extract + store the image in the raw store with its
     caption + in-text references; filter decorative/logo images by area. Cheap, do always.
  2. *Make findable* — generate a text description per figure via a local VLM that fits 8 GB
     (e.g. `moondream` ~2 GB or `minicpm-v` 8B Q4 ~5.5 GB), loaded sequentially with the text
     model at ingest (unbounded-time, §2.4). Store as a **first-class retrievable unit**:
     `figures` table + `figure_vectors` vec0 (mirrors propositions, decision A).
  3. *Make interpretable* — at generation (Stage 7), include the **actual figure image** in the
     Claude call when a retrieved unit references it; Claude is multimodal, so the precise
     interpretation happens there for ~free (the local VLM only has to make it *findable*).
  **Schema decision (settle before bulk ingest, like decision A):** add the `figures` +
  `figure_vectors` tables.
- **Scanned / image-only PDFs.** Detect low text-density pages → OCR fallback (else the doc
  ingests as near-empty and is invisible to queries).
- **Entity resolution / alias-canonicalization.** Collapse `name+type` near-duplicates
  (`LTI model` vs `Linear, Time-invariant (LTI) models`). Mainly unlocks the cross-domain
  Obsidian graph (§14) — duplicates fragment the "linked ideas" connections; invisible to
  direct Q&A. Coordinate with §14's deferred alias decision.
- **Content-hash normalization** (§11.F) — normalize whitespace/formatting before hashing so a
  re-exported PDF does not duplicate.
- **(Measure first)** Per §11.B, route the proposition/synthesis passes to the Claude API only
  if the eval (`locus eval`) shows local quality insufficient — the raw-chunk fallback above
  makes local "good enough" in many cases.

### 15.2 Stage 6 — retrieval hardening (NOT re-ingest-bound; build into Stage 6)
These operate over existing data, so they can be added/changed anytime without re-ingest.
- **Hybrid lexical + dense retrieval.** Add SQLite **FTS5/BM25** over chunk text beside the
  vector search. Dense embeddings (general-purpose nomic) blur exact symbols, tickers, and
  specific method names; lexical catches them. A retrieval miss is **not Claude-recoverable**,
  so this is the highest-ROI retrieval fix — fold it into the Stage 6 build.
- **Cross-domain retrieval mode / multi-query expansion.** The killer use case (engineering ↔
  quant synthesis) needs non-obvious cross-domain links surfaced; pure relevance reranking may
  miss them. Claude can synthesise the bridge *only if both sides are retrieved*.
- **ANN index** when the brute-force KNN count-warning fires (§11.D).

### 15.3 Sequencing
1. Prove the **Stage 6 + 7** spine on the current corpus (fold in 15.2 hybrid lexical). ✅ done.
2. Then **Stage 5.1** (15.1 + 15.4) before the multi-year bulk ingest — re-ingest-bound work must
   be locked in before the expensive pour. Figures span 5.1 (store + describe) and 7 (multimodal
   generation).

### 15.4 Re-prioritised for the broad personal-KB goal (§1)
The §1 reframe (entire personal KB — projects, achievements, history — to query, **link**, and
**feed Claude**) shifts what matters most before bulk ingest. Priority order:

- **Format coverage (do first).** The KB is heterogeneous; only PDF ingests today. Add a
  text/markdown extractor (`.md`/`.txt` — cheap, deterministic, and `vault/notes/` already
  expects it), then `.docx`. You cannot bulk-ingest a personal KB that is mostly notes and
  write-ups if the pipeline only reads PDFs. *Ingest-capability gate — before the pour.*
- **Temporal + category metadata (re-ingest-bound schema decision).** Personal/historical
  content has meaningful *dates* (when an achievement happened, when a project ran) and *kinds*
  (paper / project / achievement / note / cv). Add `source_date` + `category` to `documents`
  (+ retrieval facets) so "what did I work on in 2023" and "show my projects" work. Settle the
  schema before bulk — backfilling dates across a multi-year ingest is the expensive redo.
- **Entity-alias resolution (elevated).** "Link" is now a primary use, and cross-document entity
  co-occurrence is the linking substrate — duplicate/variant entity names fragment exactly the
  connections this goal is about. Promote from §14's deferred view-layer note to a real pass.
- **The Claude-context surface (primary use #3, not re-ingest-bound).** Decide the interface for
  feeding the KB into Claude — an **MCP server** exposing `retrieve`/`query` is the natural fit,
  or a thin retrieval API. Plan it now (shapes utility); build after the spine, independent of the
  bulk ingest.
- **Retrieval eval (Layer 3).** Now spans heterogeneous content — the definitive check that
  query+link quality holds across the broad corpus. Run before trusting bulk results.
- **Math-aware extraction + figures (15.1).** Still needed for the *technical subset*; slightly
  lower priority than format coverage, which touches the whole KB.

---

## 16. Near-term development plan (current — supersedes the §10 slice list)

> Human-readable version with checkboxes + the MCP architecture note: **`PLAN.md`** (repo root).
> Keep the two in sync.

Critical path (ingest → query) is done on PDFs. This is the ordered plan to reach a usable,
broad-corpus system, weighted per the owner's direction: **MCP server is a primary deliverable;
temporal metadata is high; math + figures are medium but kept (most of the corpus is PDF), done
before the bulk pour. Target formats: PDF (done), DOCX, slides (PPTX), code repos; later YouTube /
podcast transcripts.**

**Ordering principle:** lock *re-ingest-bound* work before the bulk ingest (changing it later
forces re-ingesting the whole corpus); deliver daily utility early (MCP); broaden formats;
validate; then pour. Each block is roughly a work-day; `[re-ingest-bound]` = must precede bulk.

**2026-06-04 corpus evaluation (re-ordered steps 3–7).** An external evaluation over the MCP
server found, verified against the code: math destroyed at extraction (broken font CMaps *drop*
ligatures — unrecoverable post-hoc — and Colab exports lose formulas entirely; corruption also
defeats the `has_math` regex, so the math-OCR pass would miss its targets); sectioning
over-fragmentation (font heuristic accepts sentence-shaped lines as headings: 109 sections /
78 pp; ToC pages ingest as content); pass-hygiene gaps (all-empty synthesis is schema-valid and
ships; meta-propositions; entity noise); and retrieval-selection gaps (one section can occupy
three rerank slots as proposition+chunk+section; pure-relevance top-8 collapses onto one
document, breaking cross-domain synthesis). Date/category facets were already fixed in code —
the corpus predates migration 0003; re-ingest activates them. Remediation: selection fix first
(not re-ingest-bound, daily utility), then all re-ingest-bound fixes before the *one* re-ingest
of the small corpus, re-evaluate against the 2026-06-04 baseline, then resume breadth. The §11.C
model benchmark stays deferred until extraction is fixed (both models currently summarise
math-stripped text). Details per step in `PLAN.md`.

1. **Temporal + category metadata** `[re-ingest-bound]`. Migration: `documents.source_date` +
   `category`. Extraction: PDF metadata creation date → file mtime fallback; category by
   drop-folder convention / heuristic. Retrieval facets (`--since`/`--until`/`--category`);
   `audit` shows the date/category distribution. *Lock the schema first.*
2. **MCP server.** Expose `retrieve` / `query` (+ `list` / `inspect`) as MCP tools (`mcp` SDK) so
   the vault is callable from Claude Code, the desktop app, and the API. Immediate daily utility
   over the current corpus; not re-ingest-bound. **Architecture (local Claude ↔ server corpus):**
   the MCP *server* runs server-side (needs DB + sqlite-vec + Ollama + reranker); the local Claude
   is the client. Use **stdio over SSH** — Claude spawns `ssh locus-server "… uv run locus mcp"`,
   tunnelling stdio over the existing SSH (no open ports/auth). Expose `retrieve` as the core tool
   so the local Claude pulls the KB as context and generates itself (needs no server-side Claude
   key). See `PLAN.md`.
3. **Retrieval selection** (eval phase A; *not* re-ingest-bound) — **done.** In
   `retrieve/rerank.py`, `select()`: cap units per `(section_id, kind)` at 1 after scoring;
   demote section-summary candidates already represented by a child (expansion re-attaches the
   summary); per-doc diversity cap (config `per_doc_cap`) — all soft, with fallback refill so
   top-k stays full. Citations deduped in `assemble.py`. Verified live: the Fourier
   signal-processing↔PDE bridge query surfaces 3 documents (was 1).
4. **Sectioning + front-matter** `[re-ingest-bound]` (eval phase B) — **done** (effective on
   the step-7 re-ingest). Deterministic, in `extract/pdf.py`: printed-ToC pages excised at
   extraction (dotted-leader density; `ExtractedDoc.toc_pages` audit trail — excision chosen
   over an `is_frontmatter` tag: stronger, no schema change); `_plausible_heading` shape
   filters on font-heuristic candidates; sanity cap `max(8, 1.5×pages)` post-filter.
   `MIN_SECTION_CHARS` deliberately stays 400 — measurement showed fragmentation came from
   bogus headings, not the size band (PDE doc: 109→37 sections, median 745→2540 chars).
5. **Pass hygiene** `[re-ingest-bound]` (eval phase C) — **done** (effective on the step-7
   re-ingest). Anti-meta proposition prompt + deterministic post-filter in `propositions.py`
   (fragment/too-short/meta/dropped-formula/title-echo; one bounded retry when everything is
   rejected); *semantic* synthesis validation (blank fields fail pydantic validation → repair
   loop → quarantine); entity surface normalization + equation-label/bare-symbol filters +
   evidence-based plural merge in `entities.py`; `audit` re-applies the same predicates to
   stored rows (QC line). Verified on the live corpus: finds doc 19's empty synthesis,
   123 suspect propositions, 209 noise entities.
6. **Math-faithful extraction** `[re-ingest-bound]` (eval phase D; promoted from late-plan).
   Corruption detector first (ligature-loss signatures + math-font evidence from span fonts —
   also fixes `has_math`); route corrupted/math-dense pages to an OCR-to-markup model chosen
   empirically on flagged corpus pages (Nougat-small / texify / Marker / GOT class); native
   `.ipynb` extractor (`nbconvert` → md); prefer source formats over PDF exports. See §15.1.
7. **Validate → re-ingest → re-evaluate** (eval phase E). Extend `locus eval`: math-fidelity
   (gate metric), proposition-entailment sampling, labelled recall@k incl. cross-domain queries.
   `--reingest` the corpus (activates `source_date`/`category`); re-run the evaluation against
   the 2026-06-04 baseline. Only then run the §11.C local-model benchmark.
8. **DOCX + Markdown/text extractors.** `python-docx` (heading styles → sections) and `.md`/`.txt`
   (headings → sections). Route in `_source_type` + watcher. Unblocks notes / write-ups / docx.
9. **Slides (PPTX) extractor.** `python-pptx`: per-slide text + speaker notes → sections; slide
   images feed the figures pipeline (block 11).
10. **Code-repo ingest.** `extract/code.py` via `python-ast` (functions, call graph, per-file
    sections; `source_type='code'`); a repo-directory entry point (ingest a repo, not one file).
    Provenance (`file_path:line`) is already in the schema and retrieval.
11. **Figures** `[re-ingest-bound]` (PDF/slides quality, medium). `figures` + `figure_vectors`
    schema; extract/store images + captions; optional local VLM (moondream / minicpm-v) →
    retrievable; multimodal Claude at generation (query.py passes the figure image). See §15.1.
12. **Entity-alias resolution + Retrieval eval (Layer 3) → BULK INGEST.** Canonicalise entity
    names cross-document (the *link* substrate); run the heterogeneous-corpus retrieval eval on
    a sample including quant-finance papers + ≥1 code repo; then pour the corpus.

**After the pour (not re-ingest-bound):** ANN-index warning (§11.D), Obsidian projection (§14),
YouTube/podcast transcript ingest, broader retrieval tests.

New dependencies introduced along the way: `mcp`, `python-docx`, `python-pptx`, `nbconvert`, and
(for math) an OCR-to-markup model (benchmarked, step 6). Keep heavy/optional ones (VLM, math-OCR)
behind extras like `[rerank]`.











































































