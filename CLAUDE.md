# CLAUDE.md — Locus

> Drop-in context for Claude Code. Authoritative for architecture and conventions; where it
> conflicts with ad-hoc instructions in chat, ask before deviating. Development history
> (steps 1–12, benchmarks, remediation passes) lives in git history and commit messages.

---

## 1. What Locus is

A **self-hosted system for querying, linking, and serving as Claude's context over the
owner's entire personal knowledge base**: papers, lecture/seminar notes, code repos, slide
decks, project write-ups, achievements, historical records.

**Three primary uses (all equal):**
1. **Query** — grounded, cited answers over the whole corpus.
2. **Link** — surface connections *across* the corpus (cross-domain, cross-project, cross-time).
3. **Give Claude context** — retrieval/memory backend feeding the owner's knowledge into
   Claude on demand (live today via the MCP server).

**Design objective:** maximise *retrieval answer quality* per query. Ingest cost (time,
compute) is effectively unbounded; retrieval latency and answer quality are the only
constraints that matter. Workflow is query-driven, not browse-driven; no GUI.

## 2. Current state (2026-06-09, post bulk pour + round-6 desktop audit)

Steps 1–12 done, and the **BULK POUR is complete**: 33 → **306 docs** (coursework 246,
career 27, project 15, paper 13, note 5) — multi-year Oxford engineering coursework,
quant/CS papers, tracked code repos, slides, career/CV. Full ingest→retrieve→generate spine
for PDF/DOCX/MD/TXT/IPYNB/PPTX/code-repos; math-faithful OCR; figures as a first-class
multimodal unit; entity-alias link substrate (cross-doc canonicals **2,217**); MCP server;
four eval suites + deterministic audit. **384 tests.**

Post-pour work landed this round (on top of the pour):
- **Titles** — `locus retitle` (NEW, billed/cached, `locus/retitle.py`): corpus-level
  distinctive `[Module — ][Seq:] Topic` titles, collision-broken to **0 duplicates**; code
  repos lead with the project name. (At ingest, lecture series + generic-metadata exports
  had collapsed to one shared title; this is the fix. Title backup at `vault/title_backup*`.)
- **Incremental repo re-ingest** (NEW `locus/repo_sync.py`): a commit re-prepares only the
  files it changed (blob-manifest diff vs the stored manifest), not the whole repo. Repo
  watching is now a **separate `locus watch-repo`** process, mutually exclusive with
  `locus watch` (incoming-only) via the shared ingest lock.
- **`.ipynb` files ingest inside code repos** (rendered to a markdown section).
- **Round-6 desktop-audit remediation:** (H1/M1) `words_alpha.txt` (the English dictionary)
  had been ingested and the synthesis pass **hallucinated** a fake doc with 4129 phantom
  entities that hung retrieval — deleted; a **pre-ingest content gate** now quarantines
  single-column data files (`textdoc._reject_if_data_dump`). (M3) the self-ingested locus
  repo is **excluded from production retrieval** (`[retrieve].exclude_source_uris`;
  `--include-excluded` overrides). Stop-entity guard enabled (`[alias].stop_doc_freq_ratio`
  0.4 — inert until an entity exceeds 40% of the corpus; IDF weighting does the live work).

Eval (post-pour, post-fix): recall@8 **0.929**, cross-domain 1.000, banner 0.000,
file_recall 1.000; math fidelity **0.869** (in band — the OCR-fallback pages are handwritten
notes / vector-drawn formulas, GOT's safe degrade-not-invent, **not** a VRAM failure);
audit QC 0 ungrounded summaries. `links_recall` is **stale** (title-substring labels broken
by retitle + corpus growth crowding the related top-5) — needs label re-curation, not a
retrieval fix.

**Known open (priority):** **H2 — cross-domain bridge.** Within-domain retrieval is strong,
but a query in one domain's vocabulary doesn't surface the other's: the entity bridge fires
(shared "state space" canonical) yet the cross-encoder demotes the semantically-distant
match below the floor. The fix is **multi-query cross-domain expansion** (§13/planned), NOT
more canonicalization. Also open: eval-label re-curation (title-id matching would be more
robust than title-substring); doc 165 §10 F1 prop reads inverted (1/14, synthesis correct —
ledgered); `[183] Aaron Rose CV` is a third party's document (confirm it belongs); `link`
and `retitle` batch their cache writes at the end (a crash wastes spend — make incremental);
ANN index (§11) when KNN latency degrades; Obsidian projection (§13); transcript ingest.

## 3. Core principles

1. **Local data ownership.** Corpus content never leaves the server. Runtime network egress
   is only the Claude API call for final generation (and the opt-in judgement passes below).
2. **Terminal-first.** The CLI and MCP server are the product surface.
3. **Pragmatic build-vs-buy.** Off-the-shelf RAG tools were evaluated and rejected as the
   engine (none does hierarchical L1/L2/L3 + rerank). Don't re-buy that engine. Obsidian is
   permitted strictly as a read-only projection layer (§13).
4. **Quality over speed at ingest.** Ingest has no time budget. Never trade ingest quality
   for throughput.
5. **Local models for ingest; Claude API for generation and judgement.** Forced by the 8 GB
   VRAM ceiling. Judgement-quality work where errors corrupt durable state (eval judging,
   alias adjudication) deliberately uses the API — an 8B model's mistakes there are the
   §11.B failure class.
6. **One schema for every source type.** All formats reduce to L1/L2/L3 so retrieval logic
   never forks per source type.
7. **Idempotent ingest by content hash.** Re-ingest of identical content is a no-op.
8. **Recoverability governs priorities.** Claude only sees what retrieval surfaces.
   Generation-time noise is recoverable (raw chunks are always co-assembled with derived
   units — keep that property). Extraction loss and retrieval misses are NOT recoverable;
   they always outrank cosmetic quality work.
9. **Derived data is regenerable, never authoritative.** Aliases, pass caches, the future
   Obsidian vault: rebuild = delete + recompute; the ingested tables are never mutated by
   derived layers.

## 4. Hardware envelope (binding)

| Resource | Spec | Consequence |
|---|---|---|
| GPU | RTX 3070 Ti, **8 GB VRAM** | THE architectural driver: local LLMs ≤ ~8B quantised; ingest local, generation API; strict VRAM choreography (§7). |
| CPU | Ryzen 5 5600X | Cross-encoder reranker runs here. |
| RAM | 32 GB | SQLite + Ollama host overhead. |
| Storage | 1 TB SSD | Flat raw store + single SQLite DB file. |

## 5. Tech stack

| Layer | Tool |
|---|---|
| Extraction | `pymupdf` (PDF), `python-docx`, `python-pptx`, stdlib (md/txt/ipynb), `ast` (code) |
| Math OCR | GOT-OCR-2.0 (benchmark-chosen on risk asymmetry: degrades, never invents) |
| Figure VLM | qwen2.5vl:7b — via Ollama, or `llama-server` (Vulkan) for GPU vision encode (13x) |
| Ingest LLM | `qwen2.5:7b-instruct-q5_K_M` via Ollama (benchmark-chosen vs llama3.1:8b) |
| Embeddings | `nomic-embed-text` via Ollama — **768-dim, locked** (change ⇒ full re-embed) |
| Vector store | `sqlite-vec` (brute-force KNN) + SQLite **FTS5/BM25** lexical arm |
| Reranker | `ms-marco-MiniLM` cross-encoder (CPU) |
| Generation / judgement | Claude API (single call per query; multimodal — figure images attach) |
| Context surface | `mcp` SDK server over stdio (tunnelled via SSH) |
| Schema | SQLite + Alembic (forward-only migrations) |

Optional system deps (absent ⇒ graceful degradation + audit gap line, never quarantine):
LibreOffice `soffice` (slide renders), `llama-server` binary (fast figure descriptions).
Heavy ML deps live behind extras (`[rerank]`, `[mathocr]`).

## 6. Data model

Three levels per document, identical for every source type. Source of truth = Alembic
migrations in `locus/db/migrations/versions/` (`alembic upgrade head`; currently 0008);
`locus/db/schema.sql` is a human-reference summary. Never `ALTER` a live DB ad-hoc; never
force a re-ingest for a schema change — migrate forward.

- **documents** (L1): `content_hash` (idempotency, UNIQUE), `source_type`
  (pdf|code|video|docx|markdown|text|notebook|slides), `source_uri`, `raw_path`, `title`,
  `source_date`, `category` (paper|coursework|project|career|note — KIND of content, one
  axis; format lives in source_type), synthesis columns (`thesis/method/result/limitations`
  — discrete, queryable), `section_map` JSON, `gap_flags` JSON, `ingest_model`.
- **sections** (L2): position, title, LLM `summary`, `file_path`+`call_graph` for code.
  + `section_vectors` (vec0 768).
- **chunks** (L3): ~512-token raw text; provenance: `file_path:line_start-line_end` (code),
  `video_timestamp` (video). + `chunk_vectors` (vec0) + `chunks_fts` (FTS5, trigger-synced).
- **propositions**: atomic self-contained claims, first-class + embedded
  (+ `proposition_vectors`). Decision A: the highest-signal unit must be directly searchable.
- **figures**: caption + VLM description, embedded (+ `figure_vectors`); PNGs in the raw
  store as `{hash}_fig{N}.png`, orphan-cleaned on replace/delete. Same first-class logic.
- **entities**: typed (`method|dataset|author|concept|ticker|tool|theorem|metric|
  organization|other`), section-anchored, `UNIQUE(doc_id, section_id, name, type)`.
- **entity_aliases** (step 12): DERIVED, REGENERABLE total mapping `(name,type) →
  (canonical_name, canonical_type)` + cluster_id + tier; built by `locus link`; `entities`
  never mutated; singletons map to self so consumers inner-join.
- **pass_cache**: content-keyed LLM pass outputs (summaries, figure descriptions, alias
  verdicts) — re-ingests and alias re-runs only pay for what changed.
- **tags / doc_tags**: doc-level, normalised.

## 7. Ingest pipeline

`ingest_pipeline.py`. Idempotent by content hash; per-document transaction (prepare first,
then delete+write atomically — a failed re-ingest never destroys the prior doc); failures
quarantine the single doc and the batch continues. An advisory flock enforces **one ingest
process at a time** (Ollama contention produces spurious quarantines).

```
source file (vault/incoming/<category>/… or repo)
  → hash; existing → skip
  → raw store copy
  → EXTRACT (per type; extract/)
      pdf: sections via font+shape heuristics, ToC pages excised, per-page de-hyphenation
           (doc-vocab attested), PageFlags damage/math detector → GOT math-OCR with
           deterministic QC fallback; figure detection (raster + vector clusters,
           caption-pairing, density filters)
      pptx: one span per slide, real slide-number provenance, notes; visual-bearing
            slides rendered via soffice for the figure pass
      code: repo = doc, files = sections, def-granular chunks with line provenance,
            call graph, deterministic AST entities (no LLM passes — pass profile)
  → LLM PASSES (local qwen, schema-validated pydantic, bounded repair retries with
      temperature escalation; length-truncation-aware; LaTeX-escape sanitizer)
      section summaries (grounding guard: distinctive-stem overlap with own source,
      else deterministic fallback) · propositions (anti-meta prompt + deterministic
      rejection filters) · entities (normalisation, noise/grounding filters, plural
      merge) · doc synthesis (semantic validation; arbitrated title when extractor's
      is suspect) · gap flagging (evidence-grounded, precision-filtered)
  → FIGURES (batched after text passes — one VRAM swap per doc): VLM describe with QC +
      caption-only fallback; engine ollama or llama-server (fails closed to ollama)
  → EMBED (nomic): chunks + summaries + propositions + figure caption+description
  → WRITE all levels in one transaction
```

**VRAM choreography (hard-won, step 11.5):** evictions must be settle-polled (Ollama
delists before VRAM frees); evict ALL models before GOT; qwen2.5vl needs `num_ctx=4096` to
fit 8 GB. A split model produces identical output SLOWLY — no quality gate sees it; watch
GPU-idle + multi-core llama-server. **Run the math eval suite after any VRAM-choreography
change.**

**Hard rule:** every structured LLM output is pydantic-validated with bounded repair; the
pipeline never silently writes garbage and never aborts a batch on one bad doc.

Continuous ingest: `locus watch` (recursive incoming/ watcher, category from drop folder,
settle window, quarantine preserves subpath) + tracked-repo sync (`[repos]` config; HEAD
checked hourly, re-ingest only on new commits; `locus sync [--force]` manual).

## 8. Retrieval pipeline

`retrieve/` + `query.py`. Single Claude API call at the end.

```
embed query (nomic)
  ├─ propositions  top-10 (dense)        ├─ chunks  top-20 (dense)
  ├─ sections      top-5  (dense)        ├─ figures top-8  (dense)
  ├─ lexical FTS5/BM25 over chunks (exact symbols/tickers dense embeddings blur)
  ├─ path-anchored: code sections whose file stem is named in the query
  └─ entity-anchored: alias-aware — query naming ANY variant surfaces sections naming
     any sibling variant (canonical groups via entity_aliases; falls back to raw names
     pre-`locus link`)
merge → rerank (cross-encoder, CPU) → select() with diversity rules:
     per-(section,kind) cap, child-redundancy demotion, per-doc cap (config), all soft
     with refill; query-named files exempt; prefer_code guarantees a source unit for
     implementation-intent queries
  → confidence: calibrated floor (min_rerank_score 0.22) — flag, never filter;
     two-tier band (ambiguous|absent); facet-aware scoring suppresses the banner when
     every facet of a bridge query is covered; sub-floor noise pruned only when signal exists
  → hierarchical expansion (parent summary + doc synthesis, plain joins, no inference)
  → assemble coarse-to-fine under context_token_budget; citations deduped;
     code cites file:line, slides cite slide numbers, figures cite [figure on p.N]
  → Claude API ×1 — top-3 retrieved figure images attach as real images (downscale-guarded;
     missing image degrades to text-only)
```

Document facets: `--since/--until/--category`. Query modes are a system-prompt lever only —
they never change the retrieval pipeline.

## 9. Link layer (step 12)

`locus/link/` — runs AFTER ingest, cross-corpus, outside the ingest/retrieval spine.

- **`locus link`** builds `entity_aliases`: deterministic tiers first (casefold / punct /
  attested acronym-expansion incl. plural-chained lookups / attested cross-doc plural —
  same-type only, hard evidence), then embedding-blocked lookalike clusters (cosine ≥0.86
  AND token-Jaccard ≥0.34) adjudicated by the **Claude API** (forced tool-use). Verdicts
  cached in pass_cache ⇒ re-runs ≈ 0 API calls. Hard guards override the LLM: min-merge-len
  4, same-section co-occurrence never merges, canonical snapped to an actual member
  surface, code docs excluded from clustering, oversize clusters (>8) skipped.
- **`related_documents`** (`link/related.py`, joins-only): docs ranked by shared canonical
  entities; in `locus inspect` + MCP `inspect_document`. Stop-entity guard (`stop_doc_freq`)
  built but OFF until the pour (~0.4 × doc count).
- Manual-only by design (billed API); run after ingest batches. The retrieval arm checks the
  substrate per query, so the MCP server needs no restart after rebuilds.

## 10. MCP server (primary use #3)

`locus mcp` — `retrieve` (core, free, local-only) / `list_documents` / `inspect_document`,
plus opt-in `query` (server-side Claude call, billed; default OFF so the server never
advertises a billable tool unless the owner opts in). Figure images return as MCP image
content blocks (`mcp.include_figure_images`). Architecture: the server runs where the data
is (DB + sqlite-vec + Ollama + reranker); local Claude clients connect via **stdio over
SSH** — `ssh <host> ".../uv run locus mcp"` (absolute path: non-interactive SSH shells
don't source .zshrc). No open ports, no auth surface, no server-side key needed for
`retrieve`.

## 11. Evaluation & quality system

Four suites (`locus eval --suite judge|math|retrieval|full`) + a deterministic audit:

- **audit** (`locus audit`, no API): per-doc QC re-applying ingest hygiene predicates to
  stored rows (suspect props, noise/ungrounded entities, empty synthesis, corruption
  signatures, unattested numbers, OCR-fallback counter with loud warning, figure QC,
  zero-prop sections); corpus date/category distribution; gap-liveness warning; ALIAS
  SUBSTRATE block (tiers, cross-doc canonicals, suspicious merges).
- **judge**: Claude scores stored extractions against source (6 dimensions; also the
  A/B harness that settled the ingest-model choice).
- **math**: math-fidelity gate — flagged pages rendered to PNG, Claude judges stored text
  against the image. Doubles as the VRAM-regression canary.
- **retrieval**: labelled recall@8 / MRR / cross-domain banner rate / file-level recall for
  code / links_recall over labelled related-doc pairs; answer-key exclusion at the
  candidate pool (the locus repo itself is in the corpus). Labels live in
  `locus/eval/retrieval_eval.py` — **extend them whenever the corpus grows**.

Baselines to hold (step-12 gate): recall@8 1.000, cross-domain 1.000, banner 0.000,
file_recall 1.000, links_recall 1.000, judge ~4.0–4.4 band (n=8 noise), math ~0.87–0.95
band. Known weakest dimension: judge entity recall (~3.4) — §11.B extraction ceiling.

**Known limits & standing decisions:**
- **§11.B — the weakest model owns high-value passes.** 8B-quantised quality is the ceiling
  on summaries/propositions/entities/VLM descriptions. Mitigated by validation + grounding
  guards + raw-chunk co-assembly (recoverable class); revisit per-pass API routing only on
  eval evidence, and revisit VLM quality at the next model generation.
- **Brute-force KNN** (sqlite-vec): fine at personal scale; add the ANN index when the
  count warning fires post-pour.
- **Content-hash idempotency is whitespace-sensitive**; normalise before hashing if
  re-export duplicates appear.
- **Context budget** enforced in `assemble.py`, truncating finest-grained first.

## 12. Repository layout

```
locus/
├── CLAUDE.md / README.md / docs/pour-runbook.md
├── pyproject.toml (uv; extras: rerank, mathocr) · alembic.ini · config.toml (+ example)
├── locus/
│   ├── cli.py            # product surface: ingest list inspect watch sync link
│   │                     #   query retrieve mcp audit eval
│   ├── config.py         # typed config; ANTHROPIC_API_KEY via env/.env only
│   ├── db/               # connection (sqlite-vec load), migrate.py, migrations/ (0001–0008)
│   ├── extract/          # base, pdf, mathocr, figures_detect, docx, pptx, textdoc, code
│   ├── ingest/           # llm (validated I/O + repair), summarize, propositions, entities,
│   │                     #   synthesis, gaps, chunk, embed, figures, llamacpp
│   ├── ingest_pipeline.py · ingest_lock.py · watcher.py · sync.py
│   ├── retrieve/         # search (all arms), rerank+select, expand, assemble, pipeline,
│   │                     #   figure_images
│   ├── link/             # aliases (tiers+guards), adjudicate (Claude), related
│   ├── query.py          # retrieve → assemble → Claude (multimodal)
│   └── mcp_server.py
├── scripts/              # benchmarks, calibration, backfills, judges (one-off, kept)
├── eval-artifacts/       # benchmark results + reports (mathocr, figures)
├── tests/                # 322 model-free-by-default tests
└── vault/                # incoming/ (watched, category folders) · raw/ · notes/ · locus.db
```

## 13. Obsidian projection (post-pour, deferred)

Read-only visualization layer rendered over SQLite — a render target, never in the
ingest/retrieval path (`locus/export/obsidian.py` when built). Invariants: one-way export
to `vault/obsidian/` (gitignored, regenerable, never read back); authored notes in
`vault/notes/` are an input, the export is an output; joins-only. Nodes: doc notes
(synthesis frontmatter) + **canonical** entity notes (join through `entity_aliases` — never
raw surfaces); sections are headings; chunks/propositions are not nodes. Edges from
existing data only (mention, co-occurrence, shared entities). The exporter owns only its
subtrees and never touches `.obsidian/`.

## 14. Coding & operational conventions

- Python 3.11+, uv-managed; type hints mandatory; small explicit functions; docstrings
  state non-obvious assumptions and the *why* (this codebase's docstrings carry the
  decision log).
- Structured LLM I/O through pydantic models, never raw dicts; every external call has
  explicit error handling.
- Tests are model-free by default (injected fake clients/embeddings, seeded tmp DBs);
  guarded integration tests where a model is unavoidable. Keep the suite green.
- No secrets in code or committed config; key via `ANTHROPIC_API_KEY` (env or .env).
- All tunables in `config.toml` (`[ollama] [paths] [embed] [retrieve] [generation] [mcp]
  [mathocr] [figures] [repos] [alias]`); optional sections default cleanly.
- Operational rules: ONE ingest process at a time (flock-enforced); math suite after any
  VRAM-choreography change; `locus link` after ingest batches; quarantines are bugs to
  triage, not casualties; eval labels grow with the corpus.
- Out of scope: custom GUI, cloud storage of corpus content, multi-user/auth, local models
  > ~8B (hardware-bound).
