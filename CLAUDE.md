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

## 2. Current state (2026-06-09, post bulk pour + round-7 + production-polish round)

Steps 1–12 done, and the **BULK POUR is complete**: 33 → 306 docs, then **deduped to 292**,
then **291** after the third-party `harry_thoughts.docx` was removed this round
(coursework 246, career **13**, project 14, paper 13, note 5) — multi-year Oxford
engineering coursework, quant/CS papers, tracked code repos, slides, career/CV. Full
ingest→retrieve→generate spine for PDF/DOCX/MD/TXT/IPYNB/PPTX/code-repos; math-faithful OCR;
figures as a first-class multimodal unit; entity-alias link substrate (cross-doc canonicals
~2,156); MCP server; four eval suites + deterministic audit. **403 tests.**

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
- **Round-7 desktop-eval remediation:** (M4) related-docs now exclude code-boilerplate
  identifiers (Alembic env API, leading-underscore privates/dunders, `test_*`) so two repos
  sharing only an Alembic setup no longer read as related (`link/related.py`). **Eval labels
  re-curated** to stable `source_uri` keys (arxiv id / repo path / module folder) — `retitle`
  had silently broken every title-substring label; `links_recall` restored via 3
  mutually-verified pairs, recall@k 0.881→**0.976**, file_recall→**1.000** (the locus-internals
  query now lifts the self-exclusion via a per-label `include_excluded`). **Career dedup**
  27→14: 8 superseded CV versions, 3 cover-letter format dupes, the third-party `Aaron Rose
  CV`, a QR-code, + a duplicate Citadel deck (project 15→14) — originals retained in
  `vault/raw`. The desktop eval's cross-domain NO-GO was a **stale MCP server** (the process
  predated the H2 fix), not a code gap — restart `locus mcp` after any retrieval change.

Eval (post-dedup): recall@k **1.000**, cross-domain 1.000, banner 0.000, file_recall 1.000,
**links_recall 1.000**, mrr 0.833; math fidelity **0.869** (in band — the OCR-fallback pages
are handwritten notes / vector-drawn formulas, GOT's safe degrade-not-invent, **not** a VRAM
failure); audit QC 0 ungrounded summaries. The former Fourier 'signals + PDEs' miss is fixed:
multi-query expansion now also does deterministic **facet decomposition** for conjunctive
('both X and Y') queries — it distributes the shared context over each conjunct and retrieves
each separately, so an under-represented facet is no longer crowded out (`retrieve/pipeline.py`
`decompose_conjunction`).

**H2 — cross-domain bridge: FIXED** via **multi-query expansion** (`retrieve/multiquery.py`):
a bridge-shaped or low-confidence query is rephrased into other disciplinary vocabularies
(local qwen) and each candidate is reranked against the variant that found it, so a match
written in another field's terms clears the floor instead of being demoted. Live: the
labelled bridge query now co-retrieves the engineering control doc (−3.25 → +1.20) with the
finance/ML papers. Tune via `[retrieve].multi_query_expansion / multi_query_k`.

**Production-polish round (2026-06-09):**
- **`locus backup` / `locus restore`** (NEW, `locus/backup.py`): WAL-safe DB snapshot via
  SQLite's online backup API + rsync `--link-dest` hardlinked raw-store snapshots (cheap
  incremental) + notes; restore is gated on `--yes` and the ingest lock. Default root
  `vault/backups` (gitignored). Closes the no-disaster-recovery gap (principle 8/9).
- **`locus status`** (NEW, `locus/status.py`): one-screen health — doc counts by
  category/type, last ingest, vector/unit totals, **alias-substrate staleness** (entity
  surfaces not yet in `entity_aliases` ⇒ rerun `locus link`), quarantine count, last-backup
  age, and the build stamp to compare against the running MCP server.
- **Eval label set grown 21→53 queries + 3→10 related pairs** (`eval/retrieval_eval.py`):
  the pre-pour set measured <10% of the 291-doc corpus, so recall@k 1.000 had gone
  uninformative. New labels cover the coursework bulk (maths/thermo/dynamics/signals/
  control), the unlabelled papers, projects, career, and notes; every label verified live.
  Two ungrounded cross-domain bridges and one non-mutual stats pair were dropped, not forced.
- **Two known-opens closed:** doc 165 §10's inverted F1 proposition (it misread "matches
  that level [0.82]" as 0.68 — deleted, prop 24707); `harry_thoughts.docx` (doc 200,
  third-party work) removed from the DB (raw original retained in `vault/raw`).

Eval (post-polish): recall@k **1.000**, cross-domain 1.000, banner 0.000, file_recall 1.000,
**links_recall 1.000**, mrr 0.843 — over the expanded 53-query / 10-pair set.

**Obsidian projection — SHIPPED (2026-06-09):** `locus export-obsidian` (NEW,
`locus/export/obsidian.py`) renders the corpus to a read-only Obsidian vault — joins-only, no
API, no figures yet (Phase 1+2: doc notes with synthesis frontmatter + section summaries,
canonical entity notes spanning ≥2 docs, `related` doc↔doc edges reusing `related_documents`,
generated `_index.md`). One-way/regenerable; the exporter owns only `docs/`+`entities/` and
never touches `.obsidian/`. Live: 290 doc + 2146 entity notes, 1369 related edges,
byte-identical re-export. View from the Mac by rsync-pulling the tree (plan §10). Design +
transport in `docs/obsidian-projection-plan.md`.

**Code concept linking — Phase 1 SHIPPED (2026-06-09):** code repos carried AST identifiers,
not the domain concepts they implement, so the owner's projects orphaned in the link graph
(6/12 linked to nothing) — failing the primary Link use case (§1.2). NEW `ingest/concepts.py`
(pass profile `concept_entities`, code-only) extracts domain concepts from a repo's NARRATIVE
(synthesis + README + file summaries, never raw code — the §11.B-safe surface) into the same
`(name,type)` space as paper entities; `locus link`'s deterministic tiers then merge them.
Backfill existing repos: `scripts/backfills/backfill_code_concepts.py` (no re-embed) → `locus link` →
`locus export-obsidian`. Result: **0 Obsidian orphans** (was 3); quant projects link to each
other (regime-ml ↔ tanker-flow ↔ downside-risk), `links_recall` 1.000 over 13 pairs. Config
`[concepts]`. **Boundary:** project→PAPER links surface but at rank 6–8 (shared=1) — top-5
reach is Phase 2 (fuzzy clustering of code-only surfaces). Design: `docs/code-concept-extraction-plan.md`.

**Known open (priority):** ANN index (§11) when KNN latency degrades (the count-warning is
still unimplemented — brute-force is fine at 291 docs); code-concept Phase 2 (fuzzy clustering
to lift project→paper into top-5); Obsidian Phase 3 (figure embeds, co-occurrence edges);
transcript ingest.

**Operational hardening:** `link`/`retitle` persist each billed verdict to `pass_cache` in
its own commit (crash mid-rebuild no longer wastes spend). `locus mcp` logs a build stamp
(`build <sha>[+dirty] (<date>)`) to stderr at startup so a stale long-lived server is
identifiable at connect time — restart it after any retrieval/pipeline change and check the
stamp against `git rev-parse --short HEAD` (or `locus status`).

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
            call graph, deterministic AST entities + an LLM domain-concept pass over the repo
            narrative (concepts.py, §1.2 Link); skips propositions/LLM-entity passes — pass profile
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
│   ├── cli.py            # product surface: ingest list inspect watch sync link retitle
│   │                     #   query retrieve mcp status backup restore export-obsidian audit eval
│   │                     #   read capture-sync capture-conversation notes-sync
│   │                     #   structure objects evolution gaps review daily daily-pull
│   │                     #   promote annotate discover device-migrate decide intent
│   ├── backup.py         # WAL-safe DB snapshot + rsync-hardlinked raw store; restore
│   ├── status.py         # `locus status` health summary (counts, alias staleness, backups)
│   ├── health.py         # did the nightly work happen, and what did it cost
│   ├── config.py         # typed config; ANTHROPIC_API_KEY via env/.env only
│   ├── db/               # connection (sqlite-vec load), migrate.py, migrations/ (0001–0027)
│   ├── extract/          # base, pdf, mathocr, figures_detect, docx, pptx, textdoc, code
│   ├── ingest/           # llm (validated I/O + repair + per-pass routing), summarize, propositions,
│   │                     #   entities, concepts (code domain concepts), synthesis, gaps, chunk, embed, figures, llamacpp
│   ├── ingest_pipeline.py · ingest_lock.py · watcher.py · sync.py · notes_sync.py · repo_sync.py
│   ├── retrieve/         # search (all arms), rerank+select, expand, assemble, pipeline,
│   │                     #   figure_images · threads (his own threads, joined on)
│   ├── link/             # aliases (tiers+guards), adjudicate (Claude), related · threads ·
│   │                     #   projects (which project a piece of his writing is about, §24) ·
│   │                     #   connect (the written reason two documents connect; bridges, §25)
│   ├── export/           # obsidian.py — read-only vault projection (§13, joins-only)
│   │  ── agent layer (§15) ──
│   ├── agent/            # claude.py (the claude -p runner) · journal.py (agent_runs) ·
│   │                     #   budget.py (cost ledger) · state.py (objects/links/positions/acceptance) ·
│   │                     #   compose_daily.py (the §9 daily reMarkable page — aggregate-only) ·
│   │                     #   pull_daily.py (annotated-page pull-back: route, five-way bless) ·
│   │                     #   promote.py (developed threads -> vault/notes -> the corpus)
│   ├── capture/          # remarkable · transcribe · fillin · loop_a · conversations · intent ·
│   │                     #   rmdoc (.rmdoc stroke geometry) · annotate (Loop B text linking) ·
│   │                     #   device_migrate (the one-off /Daily /Reading /Notes /Admin move)
│   ├── decide/           # queue.py (what is pending + WHICH SURFACE owns it) · app.py (Textual)
│   ├── structure/        # propose.py — gated object + belief proposal (plan/apply split)
│   ├── evolve/           # trajectory.py — dated position chain + advisory tension detection
│   ├── learn/            # gaps.py · practice.py · review.py (SM-2, enrolment, stored questions)
│   │                     #   · reread.py
│   ├── surface/          # grounding.py · critique.py · synthesise.py (the §8.4 MCP surface)
│   ├── enrich/           # related.py — grounded `> [!ai] Related` owned blocks
│   ├── reading/          # md2pdf · deliver_remarkable (`locus read`) · proposals (lifecycle) ·
│   │                     #   deliver (push + OA fetch) · watch (the accept signal) ·
│   │                     #   relevance (what a book YOU added links to) ·
│   │                     #   accept (accepted -> corpus) · sweep (read the ink back)
│   ├── discover/         # arxiv · openalex · queries (what to search for) · rank · judge ·
│   │                     #   citations · why (the written reason, billed) — §16
│   ├── vault/            # writer.py (owned blocks, atomic, provenance) · markers · sidecar
│   ├── query.py          # retrieve → assemble → Claude (multimodal)
│   └── mcp_server.py
├── scripts/              # one-off, kept: backfills/ benchmarks/ reingest/ (+ scripts/README.md)
├── eval-artifacts/       # benchmark results + reports (mathocr, figures)
├── tests/                # 1002 model-free-by-default tests (tests/conftest.py pins pass_routing local)
└── vault/                # incoming/ (watched, category folders) · raw/ · notes/ · backups/ · locus.db
```

## 13. Obsidian projection (SHIPPED — Phase 1+2)

Read-only visualization layer rendered over SQLite — a render target, never in the
ingest/retrieval path (`locus/export/obsidian.py`; `locus export-obsidian`). Invariants:
one-way export to `vault/obsidian/` (gitignored, regenerable, never read back); authored
notes in `vault/notes/` are an input, the export is an output; joins-only. Nodes: doc notes
(synthesis frontmatter + section-summary body) + **canonical** entity notes (join through
`entity_aliases` — never raw surfaces; emitted only when a canonical spans ≥`min_cluster_docs`
exported docs); sections are headings; chunks/propositions are not nodes. Edges: `mention`
(doc→canonical entity) + `related` (doc↔doc, reusing `related_documents` — same ranking as
`locus inspect`). The exporter owns only `docs/`+`entities/` and never touches `.obsidian/`;
it prunes stale notes within those subtrees only. Deterministic (byte-identical re-export:
stable id-suffixed slugs + sorted iteration). Manual-only, like `link`/`retitle` — run after
`locus link`; warns + degrades to a docs-only graph if the alias substrate is absent. Config:
`[obsidian]`. **Transport to the Mac** (where Obsidian's GUI runs): rsync-pull the tree,
`rsync --delete --exclude '.obsidian/'` (mirrors the ownership invariant) — plan §10.
**Deferred to Phase 3:** figure embeds + entity↔entity co-occurrence edges (plan §9).

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
  [mathocr] [figures] [repos] [alias] [retitle] [concepts] [obsidian] [reading] [daily] [agent]
  [capture] [ingest] [structure]`); optional sections default cleanly.
  `[structure].belief_source_categories/_types` and `[capture].idea_project_fit_floor` decide
  what counts as HIS writing and when an idea links to a project (§24) — change them together.
- Operational rules: ONE ingest process at a time (flock-enforced); math suite after any
  VRAM-choreography change; `locus link` after ingest batches; quarantines are bugs to
  triage, not casualties; eval labels grow with the corpus.
- Out of scope: custom GUI, cloud storage of corpus content, multi-user/auth, local models
  > ~8B (hardware-bound).

## 15. Agent layer — capture, structure, learn (Phases 1–2 SHIPPED)

Spec: `docs/obsidian-agent-layer-plan.md`. Turns Locus from a RAG engine into the owner's
learning / project-development / interview-prep tool. **Agent state lives in its own tables and
never mutates the ingest spine** (principles 7–9); the corpus stays immutable and regenerable.

**Phase 1 — capture density (shipped).** `documents.maturity` (rough|tidy, migration 0009) +
`[retrieve].rough_penalty` (a SUBTRACTIVE penalty on the cross-encoder score — flag/down-weight,
never filter; **eval-tuned 1.5 → 0.0 = OFF on 2026-07-29**, see below) · `agent/` foundation:
`claude.py` is the ONE `claude -p` runner (env-SCRUBBED so
it uses subscription OAuth, not the metered `.env` key — this bug silently rerouted billing in
Phase 0 and again in `link/adjudicate.py`), `journal.py` (agent_runs, migration 0010),
`budget.py` · `vault/writer.py` owned-block protocol (`<!-- locus:ai:<kind> -->` markers, atomic
temp→fsync→replace, sidecar on conflict) + incremental `notes_sync` · **Loop A** (reMarkable
handwriting → Sonnet vision transcription → Haiku fill-in → grounded Related block → rough note
→ ingest) · **Loop C** (conversation capture: MCP `capture` tool, CLI, `.jsonl` importer) ·
per-pass ingest routing (`[ingest].pass_routing`, metered SDK not `claude -p`).

**Phase 2 — the value surfaces (shipped).** Migration **0011**: `objects`, `object_links`,
`belief_positions`, `review_schedule`, `acceptance_log`.
- **`structure/propose.py`** — agent PROPOSES objects + belief positions; the owner blesses
  (`locus objects --bless`). `upsert_object` never writes `status`; body merges additively so a
  re-proposal cannot delete a thread the owner tracks. **The precision bar is the module**
  (failure mode #7): concepts must resolve through `entity_aliases` to a canonical spanning
  ≥`min_concept_docs` (2) documents AND survive `link/related.non_topical_names()` (the round-6/7
  boilerplate + code-symbol filters, shared so the two layers agree what a concept is); projects/
  readings must anchor to a real `source_uri`; every object carries ≥1 grounding link; ≤3 per
  document; **belief positions only from owner-authored categories** (`[structure].
  belief_source_categories`, default `note`) with the stance sharing ≥60% distinctive vocabulary
  with the source. Plan/apply split ⇒ `--dry-run` genuinely writes nothing; use it before any
  corpus-wide run.
- **`evolve/trajectory.py`** — the headline capability. Dated position chain ordered by
  `dated_at` = the SOURCE note's date, rendered as a pure join. Tension detection is
  retrieve-then-judge (cosine alone cannot separate agreement from contradiction — they are
  equally near); a verdict citing a claim the judge was not shown is DROPPED.
- **`learn/`** — `gaps.py` (deterministic, no model: the strong signal is a concept the project
  implements that he has never written about in his own words), `practice.py` (questions only
  from stored propositions, which stay as the reference answer), `review.py` (SM-2).
- **`surface/`** — `critique` / `synthesise`: ground in-process, hand evidence to `claude -p`,
  then DROP any claim citing an evidence key it was not given.

**MCP tools added:** `capture` · `critique` · `synthesise` · `objects` · `evolution`. These call
a model but through `claude -p` (SUBSCRIPTION); `query` remains the only tool that bills
`ANTHROPIC_API_KEY`, and stays opt-in. **Restart `locus mcp` after any surface change.**

**Invariants (non-negotiable):** propose-never-mutate · grounded-or-silent (every link, critique
claim, practice item and tension cites a real retrieved unit or does not appear) · provenance on
everything (`author: agent`, `source_run`) · `_generated/` is corpus-excluded and is never read
back by the structurer (no feedback contamination).

**Eval re-curation + `rough_penalty` OFF (2026-07-29).** After the quant-focus prune (306 → 203
docs) the label set was re-checked against what the corpus is now FOR. Only ONE label had broken
(Buckingham pi — its target was pruned and no surviving doc teaches the theorem, so it was
RETIRED, not repointed); the real defect was balance — 38% of queries targeted coursework while
**all 12 captured handwriting notes were unmeasured**. Added 7 note queries + 5 note related-pairs
(each verified live); retired the one related pair the layer no longer produces (`tanker-flow` ↔
`downside-risk`, direction-asymmetric after the concepts backfill). **60 queries / 17 pairs:
recall@k 0.983, links_recall 1.000 (34/34), mrr 0.802, cross-domain 1.000, banner 0.000,
file_recall 1.000** — the one miss is a deliberately-retained known-failing label (below), and
recall over a set built largely from verified-passing labels is a RE-BASELINE, not a gain. The
labelled set is also NOT fully deterministic: multi-query expansion rephrases via local qwen, so
borderline targets move between runs — promote a label only on repeated observation.
Then `[retrieve].rough_penalty` was
swept (14 queries × 4 values) and taken **1.5 → 0.0**: recovery 2/6 → 3/6 while non-note queries
stayed 8/8 correct at every value, so the penalty prevented no intrusion and only cost recall. The
premise was wrong, not the magnitude — `rough` measures POLISH, retrieval must rank on VALUE, and
a jotted idea exists in no other document. **Residual, and the real limit:** 3 of the 6 rejected
candidates are unchanged at EVERY penalty value — dense idea-list notes whose generated summaries
flatten into generic prose, so they never enter the candidate pool. That is a §11.B extraction
ceiling (fix: stronger summary pass for `maturity=rough`), not a ranking problem; those three
queries live in `retrieval_eval.py` as its acceptance test.

**Phase 3 — the two-way daily page (SHIPPED 2026-07-30).** The loop the whole agent layer was
for: `locus daily` composes an aggregate-only page (migration 0013 `daily_pages`/`daily_anchors`/
`annotations`) and pushes it as an annotatable PDF; `locus daily-pull` reads it back and routes
the handwriting. Both on systemd timers.
- **Transport (hard-won, twice).** The pull-back reads the CLOUD copy: `rmapi get` → `.rmdoc` →
  composite the strokes back onto the PDF (`capture/rmdoc.py`). The device's render endpoint
  composites ink for NOTEBOOKS only — for an UPLOADED PDF it returns the original, and every
  Locus-delivered page is an uploaded PDF, so the tailnet-staged copy is always blank. `.content`
  carries **two pagemap schemas** (`cPages[].redir.value` and formatVersion-1 `pages` +
  `redirectionPageMap`); `rmapi put` writes the older one, so missing it dropped every stroke
  layer on every delivered page. The spend guard keys on a **stroke fingerprint**, not the
  rendered bytes — compositing is not byte-reproducible, so a file hash re-pays vision every run.
- **Loop B (`capture/rmdoc.py` + `capture/annotate.py`, migration 0016).** Book/PDF annotations
  read from the same `.rmdoc` and linked to the exact passage by GEOMETRY, not vision: shape
  decides the gesture (underline / bracket / margin note), position only sets `in_margin`; hand
  underlines sit BELOW their glyph boxes. Live: 26 marks on *Advanced Portfolio Management*.
  Migration 0016 also adds the **`idea`** object type — what reading actually produces.
  **`--transcribe`** (`capture/mark_text.py`, billed) then reads the HANDWRITING beside each
  mark: the ink is rendered on its own (marginalia falls outside the page rect, so cropping a
  composite silently loses the longest notes), and stroke count — not `kind` — decides what
  costs a call (the book's distribution has an empty band, 2 -> 13). Live: 16 notes read,
  paired with the passages they object to.
- **Writing is CONTENT, not a checkbox.** The first real page settled this: he wrote three times,
  all three questions, none in the box labelled for questions. `classify_writing` (deterministic)
  routes handwriting under ANY region into an owner-owned `question`/`idea` object grounded in
  that region (`raised_by`). A question on a recall line no longer counts as a recall attempt.
- **Read-next ranks by RARITY** (`learn/reread.concept_weight`, 1/log2(1+doc_freq)), not by a raw
  count of gaps closed — that had handed the slot to coursework (`frequency response` 18 docs,
  `eigenvector` 13, vs the quant gaps at 1 doc each).
- The **acceptance flywheel is closed**: `link/related.acceptance_factors()` folds judgments into
  related-doc ranking (it had 32 recorded and zero callers).

- **His own thinking circulates (2026-07-30).** Two halves of one loop. A **"Still open"** section
  (`compose_daily.build_open_threads`) offers back his `active` question/idea objects, least
  recently touched first — they were previously written to `objects` and never shown again, since
  the only section reading objects is the blessing queue and that reads `proposed`. Same gesture
  vocabulary as blessings: tick resolves, cross drops, writing DEVELOPS (appends, never replaces —
  overwriting would destroy the record `evolve/trajectory.py` reads). Then **`locus promote`**
  (NEW, `agent/promote.py`, free/local, also automatic after `daily-pull`) writes any thread
  carrying his development out to `vault/notes/threads/` as an ordinary note, where `notes_sync`
  ingests it — so it embeds, links, and can come back as a connection. **Only fields carrying an
  `_owner_edits` marker are written**: agent rationale must never re-enter the corpus as his
  (invariant 5), and a thread's body legitimately holds both. The render is deterministic and
  content-only (a promotion timestamp in the frontmatter would re-ingest and re-embed every
  thread note on every hourly run), and promotion bookkeeping is NOT an owner edit.

**Concept fragmentation is NOT the defect it looked like (measured 2026-08-02).** 16.6% of
canonicals span ≥2 documents, and the two candidate promotion tiers both fail on evidence:
sub-phrase promotion covers only 7.1% of singletons and is mostly wrong in both directions
(`LLMs` ≤ `multimodal LLMs`, `Country` ≤ `country risk` — the specific is not the general);
cross-TYPE merging (`Black-Scholes model` exists as concept/method/theorem/tool because an 8B
model assigns type per document and is not stable) touches 162 names and yields **+16** cross-doc
concepts, 16.6% → 16.9%. A heterogeneous 210-document corpus simply contains mostly
document-specific vocabulary; that is what the corpus is, not a bug.

What fragmentation was actually costing was measured at the CONSUMER instead, and the cause was
different: **a thread's vocabulary was its own sentence.** "interesting, can we plot this
behavior?" names no concept, because the concept is in the paragraph he was reading when he wrote
it. Including the marked PASSAGE took the eight live threads from 9 concepts named to 22, and
thread links from 1 to 2 — both of them real. Three failures found on the way, each now a test:
the document TITLE must not be included (all four ideas from one book formed a complete graph
asserting only that he read it); head words must NOT be swallowed by longer phrases (`regime` vs
`regime detection` was exactly the pair that broke); and the per-idea cap must bound the REPORT,
not the search (scanning longest-first and stopping early never reached the short concepts).

**Next:** turn a marked passage into an idea with a model pass (the geometry hands it a clean
grounded input — the `idea` type exists and nothing populates it) · note↔note surface (captured
notes form their own mutual cluster, shared=10 — the plan assumed notes link to the corpus, not to
each other) · a stronger summary pass for rough notes · concept promotion tier (**17% of canonicals
span ≥2 docs**; ~520 new cross-doc concepts measured, ~90% coursework junk, so a filter is needed
first) · **the daily page and the reading list do not know about each other** (compose_daily has
zero references to `reading_proposals`, so read-next offers corpus re-reads while real papers sit
on the tablet) · the system reports nothing about its own activity or failures (locus-maintain
failed six consecutive nights unnoticed, 2026-08-01).

**Superseded on 2026-08-03 (§25):** the standing "archive the coursework" recommendation was
measured and is WRONG — the mitigations contain it completely, and its maths bridges are the one
thing §16 keeps it for. The coursework<->quant connection is now routed and live.

## 25. The coursework question, answered by measurement (2026-08-03)

Coursework is 144 of 220 documents, 10,508 propositions (62% of the corpus) and 12,419 entities,
and six subsystems carry a workaround for that dominance. The standing recommendation was to
archive it into a separate corpus. **Measured, that recommendation was wrong on every count.**

**The mitigations all work; coursework is contained, not distorting.** `[retrieve].category_penalty`
is `{}` because a penalty of 2.0 was measured to change nothing (quant queries already return 0/8
coursework survivors). `structure/propose` gate 1d holds: **3 of 82** concept objects are
coursework-only, and all three are legitimate ML (`Mathematical Optimization`, `Stochastic Gradient
Descent`, `learning rate`). Every `connection_note` and every `object_links` thread edge ever
written is quant<->quant — **zero** coursework leakage. `learn/review` ranks paper/note first then
by rarity; `link/related` boosts the high-value categories; `learn/reread` weights by rarity.
Nothing needed loosening or tightening.

**So the real defect is the opposite of the one recorded: coursework contributes NOTHING while
costing money, and §16's founding claim had never once been delivered.** The substrate holds 2,151
cross-doc canonicals, of which 1,424 are coursework-only, 602 touch no coursework, and **125
BRIDGE** the two. Filtered by the system's own definition of a topical concept
(`non_topical_names`) plus a specificity bar, **81 survive** — `eigenvalue problem` (15 docs),
`Markov model` (20), `Bayes' theorem`, `central limit theorem`, `Frobenius norm`, `Positive
semidefinite matrix`, `Poisson process`, `linear regression`. That is exactly the "eigenvectors in
factor models vs modal analysis" transfer §16 keeps 144 documents for, sitting unread.

**The cause was not ranking and not volume: it was that the only source of connection candidates
was `_recent_capture`** — his twelve most recent handwritten notes, which are short ("does this
suggest we should we macro regime predictor..."), name few entities and name no maths at all. A
bridge could never appear however well it ranked. The bridges hang off his PAPERS and PROJECTS,
which nothing walked. Live proof: starting from those instead, 14 of 34 papers/projects reach
coursework through a substantive shared concept.

- **`compose_daily.connection_candidates`** is now the ONE pair-finder (the page and the overnight
  writer had drifted apart into two queries) and walks a second source: his papers and projects
  into `category='coursework'`. `_bridge_sources` caps at 60 rather than twelve — a bridge is rare,
  so a cap tuned like `_recent_capture`'s silently returns almost nothing.
- **Sources INTERLEAVE.** Capture leads (a connection to what he wrote this week is more live), but
  strict precedence buried the bridges: Connect fits ~1 item/day and three capture candidates sat
  ahead of them, so the first bridge would not have appeared for four days. Live rotation now:
  day 1 capture, **days 2-3 the coursework bridges**, day 4 capture.
- **`link/connect._BRIDGE_TEMPLATE`** — a coursework connection is not "something he read" but
  something he was TAUGHT, so the question runs the other way: not "should you adopt this" but
  "the maths you already have notes on is the maths this rests on; can you apply it?". Reusing the
  read template would ask whether to adopt a second-year lecture.
- **The shared concept must be in `TEACHABLE_TYPES`** — reused, not re-derived: a thing too generic
  to ask him to explain is too generic to connect on. It is what stopped an AUTHOR becoming the
  shared concept (his reading notes paired with the book they are about over `A. Denev`).
- **A refusal names the concept too**, so grounded-or-silent waved it through. Live, that stored:
  *"I don't see \"A. Denev\" mentioned explicitly in either the reading notes or the book material
  you've provided. Could you clarify...?"* — the system asking HIM for help, on his own page.
  `_REFUSAL_MARKERS` catches prose written about the task rather than to him; it fired again
  immediately on `Automatic Hedging Program`.

**Live output, which is the only evidence that counts.** Two of three bridges are good and one is
excellent: *"You learned that testing a null hypothesis involves α (Type I error) and power (1-β)
trade-offs. In your dyadic specification test, what is the null hypothesis for linearity, and how
does the node-multiplier bootstrap's nontrivial local power reflect your α/β choice?"*

**KNOWN RESIDUAL: `while loop`.** It is compound, typed `concept`, attested in prose, and passes
every bar — producing a true but worthless prompt about iterating Optibook market data. Nothing
available separates a programming construct from a mathematical one at the concept level, so it is
left rather than papered over with a name blocklist. Its real cause is that `Optibook Python
Reference` is a VENDOR manual filed under `category='project'` (§24's data wart). Also measured and
NOT acted on: code repos as bridge sources add only coursework-solutions<->coursework (his quant
repos bridge to nothing), so `source_type='code'` stays excluded.

**Eval, as a no-regression gate rather than a claim.** recall@k **1.000** (0 misses), cross-domain
1.000, banner 0.000, file_recall 1.000, links_recall 1.000, mrr 0.815 over the 68-query / 18-pair
set. Nothing here touches the retrieval pipeline, so the point is that it did not move; mrr sits
inside its usual run-to-run spread (0.799-0.843 recorded) because multi-query expansion rephrases
via local qwen and is not deterministic. Full suite 1002 passed, 5 skipped.

**The structuring backlog is NOT waste — the opposite.** Only **6 of 144** coursework documents
have ever been structured, and those 6 yielded `Black Scholes Model`, `geometric Brownian motion`,
`natural frequencies`, `frequency response`, `SGD`, `Mathematical Optimization` — precisely the
bridging concepts this section exists to surface. Letting `--unstructured --limit 20` drain the
remaining 138 (~$4, his own material ordered first) FEEDS the capability. The earlier
recommendation to stop paying for it would have optimised away the input.

## 24. Readiness audit — the paths that looked wired and weren't (2026-08-03)

An owner-chair audit before benching development: judged only by *when I write something, does it
come back, and is the page good?* Every finding came from real output — the live DB, a rendered
PDF looked at as an image, `rmapi` against the device — because the failure class here has always
been **a path that looks wired and isn't, failing silently, with tests passing either side**.
Eight such paths were found and closed.

- **The promotion loop fed itself.** `promote` -> `vault/notes/threads/` -> `notes_sync` ->
  `structure` proposed the thread AGAIN. Live: obj 79, answered and ARCHIVED, came back as obj 85
  `active` — a resolved question returned as an open one, and six more were queued for that
  night. `_is_promoted_thread` closes it (keyed on `promote.THREADS_SUBDIR`, returns before the
  model call, $0.00). `_is_generated` did not and must not catch this: a thread is HIS words.
- **The daily page rendered FIVE pages**, p2 ~90% white, on its first-ever run with the §18 code
  (the rewrite landed 14:48; the last page was built 05:32 that morning). Read is the one section
  `_lines_for` does not bound. `_MAX_IN_PROGRESS` bounds it, and the existing overflow test now
  carries live-sized content — it had passed throughout because its fixture used short reasons
  and an EMPTY in-progress list, the one shape that cannot fail.
- **Idea->project links were mostly wrong**: 3 of 4 pointed at a project the idea was not about,
  and retrieval stated it to Claude as fact (`part of: OxAI`, an exam-question generator, for a
  note about alternative data). Two causes: `discovery_profiles` holds `gap` rows as well as
  `project` rows, so a CONCEPT label usually won the search and the caller's title lookup then
  found nothing and dropped the link silently — which is why every question written on the daily
  page had no project at all; and a cosine over one handwritten line is not evidence (a leetcode
  question scored 0.830 against "AIS capture"). NEW `link/projects.py`: deterministic tier first
  (every distinctive token of the title present — requiring ALL of them stops `Swaps Momentum
  Strategy` firing on "strategy"), then `best_project` above `[capture].idea_project_fit_floor`
  (0.70; the one correct cosine link scored 0.756, every wrong one 0.637-0.673). A runner-up
  MARGIN test was built, measured, and REJECTED: it does not separate them. Now live on the page:
  `question · regime-ml · tanker-flow`.
- **`archived` means two opposite things** — a cross archives an object and so does a tick that
  resolves a question — so filtering thread linking and thread context on `status` buried both.
  `state.dropped_object_ids` reads the judgement he actually recorded, which also gives
  `acceptance_log(surface='object')` its first reader (94 rows, none).
- **The `not_understood` signal reached nothing.** `build_rereads` ran only "if a slot is spare",
  and the shelf caps at 10 against 3 slots, so never. It now holds a reserved seat — and that
  exposed the seat being worthless: `_explains` took the argmax of RAW similarity (everything
  scores 0.84-0.98), offering *Sampling, aliasing, modulation* for a note about trading
  "signals". It reranks now, against `_REREAD_MIN_RERANK` 2.5 (genuine 4.07-4.82, best wrong
  1.917); 8 of 10 marks correctly produce NOTHING, which is the honest answer.
- **A window is not a ledger** (migration **0027**, `documents.structured_at`). `--ingested-since
  "2 days ago"` lost anything arriving while `locus-maintain` was broken — six consecutive
  nights — permanently and unnameably. Live casualty: doc 482, his handwritten `Optimisation`
  note, zero objects, and a dry run proved it had two concepts to give. Stamped even when a
  document proposes nothing (else the empty ones re-bill nightly), NOT stamped on a model
  failure. The unit now runs `--unstructured --limit 20`, HIS material first.
- **Owner-authored is about PROVENANCE, not category** (his instruction: project write-ups are
  canonical, "arguably the most important documents in the project", whoever typed them).
  Everything he writes lands under `vault/notes/`, whatever category the DEVICE FOLDER assigned
  (`Notes/engineering` -> coursework), so category-keying made his own handwriting ineligible for
  an idea because of which folder he wrote it in. Path decides first; category +
  `belief_source_types` is the fallback, and both halves matter because `category='project'` also
  holds two Optibook VENDOR manuals. **This surfaced a real dependency**: `learn/gaps.py` reused
  the same setting for a different question, and the explanation gap ("a concept his project uses
  that he has never explained") would have answered itself once project docs became his — the
  object's OWN documents are now excluded, which is what the question always meant.
- **The page was read back as his handwriting.** Vision transcribes ink AND the printed text under
  it: region O2 returned `"raised 2026-07-30"`, a line Locus printed, which became obj 79's
  development and resolution, was promoted, and was ingested as his words — invariant 5 failing in
  the exact direction it exists to prevent. A transcription ENTIRELY contained in the page's own
  markdown is now dropped. Also: `HAS NEVER RUN` had no grace at all (a healthy weekly job was
  shouted about in capitals and would have been for seven mornings), and promoted notes printed
  bare object ids ("Raised against: ... 15, 95, 80") into the corpus.

**Verified good and left alone:** retrieval (his own thread returns at rank 1, carrying `part of:`
and `also touches`), mark geometry + intent (26 marks, 24 classified, sensible), ink transcription
(16 of 17 eligible; the stroke distribution confirms the threshold loses nothing), `daily_shown`,
backup/restore, `locus status`. Quarantine is benign (8 files: `uv.lock`, `pyproject.toml`, PNGs).

**The open items were then closed (same day).**

- **One blessing surface.** `locus objects --bless/--archive` removed; `locus decide` is the only
  place a status changes. `locus objects` is read-only and now prints the provenance keys
  (`from_mark`, `from_anchor`, `promoted_path`) that were written on every object and read nowhere.
- **The Think page fills.** Equal share per subsection then round-robin refill, so a day with no
  connections is no longer a two-thirds page; and marks are no longer capped ONE PER DOCUMENT — a
  hard rule that, since every mark is on the same book, capped the section at one mark a day
  forever. Measured: **3 items is what fits** (4 renders a fifth page — `_MIN_LINES["think"]` is 3
  and 4x3 exceeds the 9-line budget). Raising it means accepting 2 writing lines per item.
- **One definition of "his writing"** — `state.owner_authored_sql`, the query form of
  `propose._is_owner_authored`. The daily page's connection source and re-read ranking were still
  on `category='note'`, the same defect the proposer's gate had, in two more places. A test
  asserts the two forms agree on every document.
- **The flywheel has a reader.** `channel_stats` grouped by `why_kind`, which is `'discovery'` on
  every row ever produced — one bucket, so its per-channel breakdown could never appear. Now keyed
  on `evidence_key`, the same string `rank._cap_per_profile` uses, and `rank.subject_prior` folds
  the kept-rate into channel order gated on 4 resolved judgements (inert today, self-starting).
  The `reading` surface now stops a declined document being re-offered; the `recall` rows were
  DELETED (the SM-2 grade already records the attempt, and they were always `kept`).
- **Mark 25 recovered** — the 1 of 17 whose ink was never read: "interesting systematic portfolio
  construction". Its intent had been guessed `not_understood` from no text and was permanent,
  because `pending_marks` never revisits a mark that has one. Transcription now clears a
  MODEL-set intent when it writes a note; re-inferred live as `important`.
- **Device tree cleaned** to `/Daily /Reading /Notes /Admin` (+ the device's own `/trash`).
  Migration **0028** drops `tags`/`doc_tags` (no code either side, 0 rows).

**Still open, deliberately:** Loop C (conversation capture) has produced 0 documents — the owner
confirmed he simply has not used it yet, not a defect. `/admin` survives because rmapi case-folds
it onto `/Admin` (which holds his NDA), so it cannot be addressed unambiguously by path;
`/brevan_howard` survives because it holds one file named `Learn List/ Questions` and rmapi parses
the `/` as a path separator — both need a rename on the device first. 25 of 32 recall items lack a
written question (fills at 8/night, self-clearing).

## 23. Eval re-baseline + a PROVEN restore (2026-08-02)

**Eval, re-curated at 218 documents.** No label had gone stale — the corpus grew rather than
shrank — but recall@k had reached **1.000 with zero misses**, which is the saturated, uninformative
state §11 warns about. Eight queries added and each verified live before being added (rank in the
comment), plus one related pair:

  - the four papers the discovery loop PROPOSED and he accepted — the first labels in the set for
    material the system chose rather than material he had already collected;
  - *Advanced Portfolio Management*, the most-annotated document in the system, ingested at last;
  - **three queries over his OWN THINKING**, which the set could not measure at all until threads
    were promoted into the corpus.

Three candidate related pairs were TESTED AND NOT ADDED because the layer does not produce them
mutually — a label asserts what the layer does, not what one wishes it did. (The two regime
threads ARE linked, as objects via `link/threads.py`; that is a different substrate from
`related_documents` and is not what this metric measures.)

**60 -> 68 queries, 17 -> 18 pairs: recall@k 1.000 HELD, mrr 0.799 -> 0.804**, cross-domain 1.000,
banner 0.000, file_recall 1.000, links_recall 1.000. The honest gate is recall holding on a set
that GREW; a frozen number rising would mean nothing.

**Restore is no longer a hope.** `scripts/analysis/verify_restore.py` restores the newest snapshot
into a throwaway tree and checks it is a USABLE DATABASE rather than a file that exists:
`PRAGMA integrity_check`, row counts, and specifically the agent state that is **not regenerable**
(39 blessed objects, 4 carrying owner edits — lose those and no re-ingest brings them back). It
also confirms an older snapshot MIGRATES FORWARD, which matters because every snapshot predates
some migration: the 15:22 backup restored at schema 0022 and upgraded cleanly to 0026. The CLI's
guard was checked too — `locus restore` without `--yes` leaves the live DB's mtime unchanged.

## 22. Reporting: does it still work, and what did it cost (2026-08-02)

`locus/health.py`, migration **0026**. The failure it exists for: `locus-maintain` failed six
consecutive nights and nothing said so. The fix is not "log harder" — the logs were there — it is
that nothing ever ASKED, so silence and success looked identical.

**Almost nothing was journalled.** Only `capture` opened an `agent_runs` row, so the status line
shipped in §18 reported on ONE of nine nightly steps and called the rest healthy. Journalling now
happens at DISPATCH (`cli._journal_kind`): one place instead of nine, impossible to forget when a
command is added, and it catches a crash anywhere in the command rather than only inside a block
someone remembered to wrap.

**Three failure modes need three detectors**, which is why one check was never enough:
`error`/`degraded` (a run that broke) · a `running` row older than 2h (started and vanished — the
process died) · and **no row at all**, which is the case that hid for six nights and which no
amount of reading `agent_runs` can find. That last one is caught two ways: `EXPECTED_CADENCE_HOURS`
(a kind absent for longer than 2.5x its cadence is overdue — derived, never stored, because a
stored "next run" has to be updated by the code that is failing), and `timer_failures`, written by
`locus record` from `OnFailure=locus-failure@%n.service` on every unit — the only detector that
works when the process never reaches Python. `locus-maintain` also records its own completion,
because it is nine ExecStart lines and nothing else can say the UNIT finished.

**Spend needed a reader, not a store.** `claude -p` reports `total_cost_usd` in its envelope and
`journal` already stored it; the numbers had nothing to sum because nothing was journalled.
`locus status` now shows spend against `[agent].daily_cost_cap_usd` broken down per kind — a total
answers "how much" but never "what for", and the second is what changes a decision.

One definition of healthy: `compose_daily.build_status` delegates to the same `health.check`, so
the page and the terminal cannot disagree.

## 21. Threads: one substrate for his own thinking (2026-08-02)

Asked why ideas and objects looked like two media, the answer was that `idea` IS an object type —
but that three things around it were wrong, and all three are now fixed.

- **An idea can be born anywhere.** `structure/propose.py` could only propose
  `project|concept|question|reading`; `idea` was not in its vocabulary, so an idea jotted in a
  lecture or a speaker session was transcribed, ingested, and died as searchable text. It is now
  proposable, gated to **owner-authored categories only** (`[structure].belief_source_categories`)
  on the same reasoning as belief positions: an idea is a proposal to DO something, and one found
  in a paper is the PAPER's. The three routes in are now a mark (§20), the daily page, and a note.
- **Threads connect to each other** (`link/threads.py`, joins-only, runs inside `locus link`).
  Two threads are linked when they NAME THE SAME CANONICAL CONCEPT — a fact checkable by reading
  both, not a cosine, because embedding similarity cannot separate "about the same thing" from
  "uses the same words" and `entity_aliases` is already the system's own definition of sameness.
  Guards: a canonical must span ≥2 documents (a name in one document is that document's
  vocabulary), ≥5 characters (`ML`/`VaR` fire on everything), `non_topical_names` applies, and
  only HIS text is matched — never the proposer's `why`, or two ideas would link because a model
  repeated itself. Live: his tanker regime question ↔ his "markets are not stationary" note.
- **A thread can have a trajectory at all.** `record_position` accepts only concept/project
  subjects, so the chain he builds by hand pass-by-pass on the daily page — `body.development` —
  was the one chain `locus evolution` could not show. `state.development_positions` merges it with
  `belief_positions` at READ time: different provenance (extracted vs authored), one chain, and
  nothing copied so neither store can drift.
- **Threads reach the corpus, so `locus query` can see them.** Objects are in NO retrieval arm —
  the only bridge is `locus promote` -> `vault/notes/threads/` -> `notes_sync` -> the ordinary
  spine. `render_thread` required DEVELOPMENT passes, so a thread had to be written on twice
  before it could cross; every idea born from a margin note fails that by construction (it carries
  the sentence he wrote once, in a book, and nothing else), which left all four of the first real
  mark-born ideas permanently unreachable by query. The bar is now HIS WORDS, not his persistence
  — `owner_fields` still gates it, so a thread carrying only the proposer's rationale promotes
  nothing. Live: 6 threads promoted and ingested; "what have I thought about regime detection"
  now returns his own notes.
- **Retrieval returns the THREAD, not a flattened copy** (`retrieve/threads.py`). Promotion made
  the text findable; what came back was a note stripped of the thing that made it worth storing as
  an object — the project it belongs to, the threads it touches, how his view moved. It is joined
  on during EXPANSION, not added as a retrieval arm: every owner-authored thread is already
  promoted, so a parallel arm would put the same text in the pool twice, competing with itself for
  the top-k and double-counting against the per-doc diversity cap, and `Candidate` is
  (doc_id, section_id) which an object has neither of. The join is exact — `promote` records
  `body.promoted_path`. Live: "what have I thought about regime detection" returns his idea
  carrying `part of: regime-ml` and the tanker thread it touches.
- **An idea renders beside its project** and the threads it touches (`_thread_context`). Both
  facts were already in `object_links` and neither was ever printed, so a connected thread came
  back looking free-floating. A related fix: `object_links.target_key` for `target_kind='object'`
  is `str(object_id)`, and §20's first cut stored a profile LABEL there — every idea→project link
  pointed at nothing resolvable.

## 20. Mark intent — a mark becomes an idea (step 4 SHIPPED 2026-08-02)

`docs/daily-use-refinement-plan.md` §4. Migration **0025**. Asked what an underline means he gave
THREE answers — "something I think is important, something I dont understand, or an idea I have
linking to the content of that passage" — and until now all three got one fate: search fuel. 26
marks had accumulated and the `idea` type (migration 0016) had never once been populated.

    important        stays retrieval. Nothing is pushed at him: the mark already said it matters.
    not_understood   the ONLY thing that earns a corpus re-read a slot on the Read page. Read live
                     from the intent, so the re-read vanishes when the confusion is resolved, and
                     gated on a searchable seed (>=25 chars) — `take the` and `NMVSPY` were both
                     classified not-understood live, and a re-read seeded on noise is exactly the
                     useless suggestion that discredited the old section.
    idea             an `idea` object linked to the passage AND (via the same profile match a
                     book's relevance uses) the project it names. Lands `active`, not `proposed`:
                     the TEXT is his, only the ROUTING was inferred. Returns on the Think page and
                     reaches the corpus through `locus promote`.

**Confidence is load-bearing.** His answer was "infer, then let me correct", so below
`[capture].intent_confidence_floor` (0.6) NOTHING happens except that the mark becomes a `locus
decide` item — acting on a low guess silently is that answer with the correction removed. An
intent HE set (`intent_by='owner'`) is never re-guessed. The 12h settle window is his, so the pass
never reacts to ink he is still writing. `locus intent --dry-run` writes nothing, which is how the
whole three-way split was inspected before anything acted on it (live: 8 important, 12 not
understood, 4 ideas, 2 with no text at all).

**KNOWN GAP, and it is the important one.** This covers marks on documents in `/Reading`. An idea
written in a NOTEBOOK (`/Notes/engineering`, a speaker session, a lecture) is transcribed by Loop A
and ingested as a `note` — and stops there. `structure/propose.py` can only propose
`project|concept|question|reading`; `idea` is not in its vocabulary, so nothing extracts an idea
from his own prose. That is backwards, because rough notes are where he says the most valuable
material is. Closing it means running the same three-way pass over newly-ingested notes.

## 19. `locus decide` — the approval surface (step 3 SHIPPED 2026-08-02)

`docs/daily-use-refinement-plan.md` §3. A Textual app (`[tui]` extra), **one tab per kind**, moved
between with left/right; `y` accept · `n` reject · `e` correct · `u` undo · `q` quit · `esc`
cancels an edit. Free and local. Kinds: **proposed objects** (bless/drop), **duplicates** (two
objects that are one concept — merge or keep apart), **abandoned reading** ("no marks in 20 days —
wrong paper, or just not yet?"). Mark-intent corrections slot in at step 4.

Duplicates use two tiers: the alias substrate first (if `locus link` gave two surfaces one
canonical, two objects titled with them are one concept by the system's own definition of
sameness), then normalised titles for objects the substrate never saw — which is the tier that
fires today, on `Bootstrap`/`bootstrap`. A merge ARCHIVES rather than deletes and folds bodies
through the same `merge_body` the structurer uses, so `u` can reverse it. "Keep separate" is
recorded on the `link` acceptance surface (defined for alias adjudication, previously unused —
same kind of judgement, so one history rather than two) and never asked again.

Three layout facts, each found by using it: **tabs** exist because one scroll meant reaching a
reading decision past forty-eight concepts; the **edit box is docked**, because mounted inside the
scrolling body it rendered below the fold with 48 cards, so `e` looked like it did nothing and
every later key was a no-op while `_editing` stayed true — the reported freeze; and the
**background is `ansi_default`** throughout (`surface` and `panel` too, not just `background`), so
the terminal's own transparency shows through, the theme trick taken from `digest/tui.py`.

**THE INVARIANT, and it is his: no decision may ever appear on both surfaces.** "I should not be
able to approve the same thing on the daily page and the tui." Two surfaces that can both resolve
one item is how a decision gets lost — he ticks it on paper, clears it in the terminal having
forgotten, and the second silently overwrites the first, or the flywheel learns twice from one
judgement. The split is computed in ONE place (`decide/queue.pending`, which subtracts whatever
`compose_daily` is currently offering) and `tests/test_decide_queue.py` asserts the two key sets
never intersect. That matters beyond today's kinds: once mark-intent lands, a marked passage will
be both a Think item and — when ambiguous — a TUI decision.

Two things the queue must not become: `resolve()` writes an abandonment answer to
`acceptance_log(surface='discovery')`, the per-channel prior that already tunes what gets proposed,
because "this signal should DO something, not just be a value that sits in a table"; and `u` undoes
against the DATABASE, not just the UI, because `n` sits beside `y` in a single-key interface.

## 18. The daily page — four pages, one section each (step 2 SHIPPED 2026-08-02)

`docs/daily-use-refinement-plan.md` §2. Composition stays **aggregate-only** (no model call, so
the page renders whether or not last night's runs succeeded); the prose it prints was written and
stored earlier. Migration **0023**.

    p1 Read     from `reading_proposals` — the WHY layer over the Reading/Proposed shelf, plus
                what is in progress and the shelf's true state. The old section offered corpus
                re-reads (an Optibook manual from last year) while ten real papers sat unmentioned:
                `compose_daily` had zero references to the discovery tables.
    p2 Think    marks + open threads + connections, ONE page and one action vocabulary, with three
                subsections named for PROVENANCE (`From your reading` / `Still open` /
                `Connections found`) — the missing information was never "what kind of item is
                this" but "where did it come from". Only connections carry a tick.
    p3 Recall   the question; answers overleaf, never beside it.
    p4          the open region, then the answers small at the foot.

- **Nothing is shown twice** (`daily_shown`). A page is built every morning regardless of whether
  the last was read, so this is what makes a skipped day lose nothing and repeat nothing.
  `item_key` carries the item's VERSION (`object:41:<updated_at>`), which is what makes ONE rule
  right everywhere: a developed thread returns, an untouched one does not, a re-scheduled recall
  still recurs (spaced repetition would otherwise have been silently disabled), and a proposal
  returns only once its `why` has been rewritten — so a repeat always carries new text.
- **`/Daily` is an inbox.** `read_at` is stamped the first time ink is seen and the page is then
  moved to `/Daily/YYYY-MM`; loose pages are exactly the ones he has not been through.
- **Recall finally has a question.** `resolve_prompt` returns the PROPOSITION as the prompt — he
  was being shown the answer and asked to recall it. `review_schedule.question` stores a real
  question (`learn/review.fill_questions`, billed, overnight via `locus review --write-questions`);
  without one the page degrades to the old behaviour and prints no answer.
- **Blessings left the page** for the terminal TUI (step 3, not yet built — `locus objects --bless`
  carries it meanwhile). `_route_blessing` is retained so a page delivered before the change can
  still be pulled back.
- **Layout is a derived constraint, not a taste.** One section per PHYSICAL page; `_lines_for`
  sizes the writing space so a section fills its page and never exceeds it (~9 ruled lines at
  `[daily].rule_gap_em` 2.6). Change that gap and the budget must be recomputed —
  `scripts/analysis/render_daily_sample.py` renders a real PDF to look at, and there is a test
  asserting a full page does not overflow into a fifth.
- **The written reason** each paper is on the shelf (`discover/why.py`, billed `claude -p`) is
  composed at proposal time and REWRITTEN after 7 days against his current threads. The
  deterministic `why` is a cosine distance ("fit 0.76") — a fact about the ranker, not a reason to
  read anything. Grounded in the paper's abstract + the matched project's stored profile facets,
  and dropped rather than stored if it fails `ingest.summarize.is_grounded` against them.
- **Books he adds himself get relevance too** (`reading/relevance.py`, migration 0024, free —
  embeddings + a cosine against `discovery_profiles`). A proposed paper arrived with a project
  link and a written reason; a book he bought or was recommended arrived as a bare filename, which
  is backwards — material he sought out himself is where a cross-domain link is most interesting,
  because no ranker decided it was relevant. Scored on **the passages he marked**, not an abstract:
  an abstract says what the author thinks a book is about, the marks say which parts made HIM stop.
  Live: *Advanced Portfolio Management* -> `Alpha Fund` (fit 0.75) off its 26 marks. `in_progress`
  now reads `reading_targets`, not `reading_proposals` — which is why the one book he was actually
  reading used to be the single thing the section left out.
- **The status line** sits at the foot of p1: what ran, and loudly what failed (including a run
  that opened and never closed). Systemd-level detection is step 5.

## 17. Device layout — the reMarkable tree (step 1 SHIPPED 2026-08-02)

Design + the full daily-use refinement plan: `docs/daily-use-refinement-plan.md` (nine-round
requirements interview; §8 records where the owner overruled a proposal and why).

    /Daily     Locus writes. An INBOX — a page stays loose until it has ink on it, then
               archives to /Daily/YYYY-MM. Never ingested as a corpus document (invariant 5);
               his INK on it is fully processed by `daily-pull`, and a developed thread
               reaches the corpus as his words through `locus promote` -> vault/notes.
    /Reading   Both write. Proposed (Locus only) | In-Progress | Finished. ONE lifecycle for
               everything he reads: merging the old `/reading_list` in is what finally lets the
               annotation sweep see the most-annotated document in the system.
    /Notes     He writes, Locus ingests. Topic folders BENEATH it drive category.
    /Admin     Excluded from everything.

**The coupling to hold.** Device folder names drive ingest categories, so a rename silently
changes how his writing is filed. `capture/remarkable.topic_folder()` keys category on the folder
*beneath* `[capture].notes_root` (`Notes/engineering` -> `engineering`) and falls back to the old
top-level keying outside it, so an UNMIGRATED device behaves exactly as before and capture never
has to be taken down for the move. Map (2026-08): `engineering`->coursework, `projects`->project,
`careers`->career (was silently falling through to `note`); `quantum_ml` is deliberately no longer
coursework — it became a research internship, so it reads like `brevan_howard`.

**`locus device-migrate`** (`capture/device_migrate.py`) is plan-first and item-level: `--plan`
writes an editable `vault/device-migration.toml`, `--snapshot` pulls every document to local disk,
`--apply` moves them and REFUSES without both a clean validation and a complete snapshot. It is
item-level because `/reading_list` is not homogeneous — an annotated book and a handwritten
notebook live in it, and no folder rule can tell them apart. Documents move; folders never do (an
`rmapi mv` on a folder moves an unreviewable subtree). Anything the rules cannot decide is marked
`review = true` and blocks `--apply` until he resolves it.

## 16. Reading discovery — Phase 4 (SHIPPED 2026-08-01)

Spec + measurement log: `docs/reading-discovery-plan.md`. Proposes reading, delivers it to the
tablet, and ingests what the owner accepts. **The accept signal is a folder move**: out of
`Locus/Reading/Proposed` = accepted; deleted = a firm no; left to expire = a weak no. Migrations
**0017–0022**. Live on timers (`deploy/systemd/locus-discover-{pull,harvest}`): pull hourly (free,
local), harvest weekly (network + GPU).

- **Search, not browse.** Category browsing was RETIRED (`[discovery].browse_categories=false`):
  it capped the pool at a rolling ~3-week window, and **methods are old** — a relevance search
  returns the canonical treatment from 2008 or 2014 that no recency feed can reach.
- **Queries come from HIS material**, interleaved so a truncated budget still covers every source:
  `marked` (concepts inside a passage he underlined — the strongest signal), `reading` (other
  concepts from annotated documents), `project` (methods his projects name), `gap`. Coursework
  concepts are KEPT deliberately — eigenvectors in factor models vs modal analysis is exactly the
  cross-domain transfer this exists for.
- **arXiv + OpenAlex.** arXiv is preprints skewed to CS/physics; OpenAlex adds journals, books and
  citation counts. OpenAlex holds **no reference lists for preprints**, so the citation channel
  (`discover/citations.py`) is built but idle until a journal article is accepted.
- **Ranking**: bi-encoder fit over multi-facet profiles → cross-encoder rerank (the same
  ms-marco stage every corpus query uses) → citation prior *centred on the pool median* → a local
  model used ONLY as a floor filter, never a ranker. Caps are on the STOCK in `Proposed` (10
  papers / 1 book), so a full folder proposes nothing.
- **`reading_targets`** maps a device document to its corpus `source_uri`, which is what lets the
  marks he makes reach the ingested paper (`reading/sweep.py`, guarded by a stroke fingerprint).

**Live 2026-08-01:** 4 papers proposed → accepted → ingested (papers 14→18); pool ~1,200
candidates; 4 `kept` verdicts are the flywheel's first real data; 0 marks read back yet.
