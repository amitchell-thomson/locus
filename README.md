# Locus

**A self-hosted retrieval engine over an entire personal knowledge base — built to make
everything I've learned, built, and written queryable, linkable, and usable as Claude's
context.**

Papers, lecture notes, code repositories, slide decks, project write-ups, notebooks, a CV:
one heterogeneous corpus, ingested by local models on an 8 GB consumer GPU, served through
a hierarchical hybrid-retrieval pipeline with measured, gated quality — and exposed to any
Claude client as an MCP server over SSH.

```
locus query "How do regime-switching models in finance relate to
             state-space models in control theory?"
```

> Grounded, cited answers that bridge documents which never mention each other — a
> quant-finance paper and control-theory lecture notes — because retrieval surfaces both
> sides and Claude synthesises over the actual source text, attached figures included.

---

## Why this exists

Off-the-shelf RAG tools (AnythingLLM, Open WebUI, Khoj, NotebookLM, Obsidian AI plugins)
were evaluated and rejected: none does hierarchical multi-granularity retrieval with
reranking, none treats *figures* or *atomic claims* as first-class retrieval targets, and
none is designed for the constraint that actually shapes this system — **the corpus must
never leave my server, and the only GPU available has 8 GB of VRAM.**

That constraint forces the defining architectural split:

- **Ingest is local and unbounded-time.** Quantised 7B models (text, vision, OCR) run
  sequentially on the 3070 Ti under strict VRAM choreography. Ingest quality is never
  traded for throughput — a document is ingested once and queried forever.
- **Generation is a single Claude API call** over an assembled, token-budgeted context.
  The only corpus content that ever leaves the machine is the retrieved context for the
  question being asked.

Three equal product goals: **query** (cited answers), **link** (cross-domain connections
through a canonicalised entity graph), and **context** (an MCP server feeding the corpus
into Claude Code / Desktop on demand).

## Headline results

Measured on the live 33-document corpus (coursework, quant-finance + CS papers, 5 code
repositories, slides, CV — 782 sections, 3,876 chunks, 4,025 propositions, 389 figures,
5,948 entity mentions):

| Metric | Result |
|---|---|
| Labelled retrieval recall@8 (21 queries incl. code + figures + alias-bridged) | **1.000** |
| Cross-domain recall / confidence-banner misfires | **1.000 / 0** |
| File-level recall (the *source file* surfaces, not prose about it) | **1.000** |
| Related-document link pairs (entity-graph layer) | **4/4** |
| Math extraction fidelity (Claude-judged vs page image) | **0.93** (raw text layer: 0.73) |
| Ingest quality, LLM-as-judge, 6 dimensions | **4.35 / 5** |
| Figure description throughput (GPU vision encode vs CPU) | **13× faster at judged parity** |
| Tests (model-free by default: fake clients, injected embeddings) | **322 green** |

Every number is reproducible: `locus eval --suite full` runs all gates; `locus audit` runs
deterministic corpus QC with zero API calls.

## Key decisions

The choices that shaped the system, and why. Each is elaborated in the sections below.

| Decision | Why |
|---|---|
| **Build the retrieval engine, don't buy** | No off-the-shelf RAG does hierarchical L1/L2/L3 retrieval + cross-encoder rerank with figures and atomic claims as first-class targets. The engine is the differentiator — it's the part worth owning; everything else is glue. |
| **Local models ingest; the Claude API only generates** | The 8 GB VRAM ceiling and a *corpus-never-leaves-the-server* constraint force the split. The only content that ever leaves the machine is the retrieved context for the question being asked. |
| **One schema for every source type** | A PDF, a git repo, and a slide deck all reduce to L1 document → L2 sections → L3 chunks, so retrieval logic never forks per format. A new format is an extraction problem, never a retrieval one. |
| **Propositions and figures are first-class, embedded units** | The highest-signal content must be *searchable*, not merely assembled into context after a coarser unit matches. Figures additionally attach to the Claude call as images, so interpretation happens in the strongest model. |
| **Hybrid retrieval, then rerank** | Dense embeddings blur exact symbols and tickers, so a BM25 lexical arm plus path- and entity-anchored arms run alongside four dense granularities — all merged through a CPU cross-encoder with diversity-aware selection. |
| **Choose models by judged benchmark, on risk asymmetry** | GOT-OCR-2.0 was picked over two *higher-scoring* math-OCR engines because it degrades instead of inventing. The ingest LLM, the VLM serving path, and the mmproj quantisation were all settled by A/B against the source / page image. |
| **Quality is eval-gated and adversarially audited** | Four eval suites plus a deterministic, API-free audit gate every change. Red-team findings become permanent audit predicates or eval labels, so a fixed bug cannot silently recur. |
| **The model proposes, deterministic code disposes** | Every structured LLM output is schema-validated with bounded repair, grounding-checked against its source, and overridable by hard guards. One bad document quarantines; the batch continues. |
| **Derived data is regenerable, never authoritative** | The canonical entity graph, pass caches, and the Obsidian projection are pure functions of the SQLite store — rebuilt with one command, never mutating the ingested tables. |
| **Cross-domain links via a canonical entity substrate** | Surface variation ("LTI model" vs "Linear, Time-invariant model") fragments the very connections a knowledge graph is for; deterministic merge tiers + Claude-adjudicated fuzzy clusters (with hard guards) canonicalise them, and multi-query expansion bridges vocabulary across fields. |

## Architecture

```
                         INGEST (local, unbounded time)                    
  vault/incoming/<category>/  ───────────────────────────────────────────┐
  tracked git repos ──────────► watcher / sync (flock: one ingest at a time)
                                      │
        ┌─────────────────────────────┼──────────────────────────────┐
        ▼                             ▼                              ▼
   EXTRACT                       LLM PASSES                      FIGURES
   pdf/docx/pptx/md/txt/         qwen2.5:7b (Ollama)             detect (raster+vector,
   ipynb/code(AST)               summaries · propositions        caption-paired, density-
   ToC excision, de-hyphenation  entities · synthesis · gaps     filtered) → qwen2.5vl
   page damage detector          pydantic-validated, bounded     describe (Ollama or
   → GOT-OCR-2.0 math recovery   repair retries, grounding       llama-server GPU encode)
     (QC-guarded fallback)       guards, quarantine-not-crash    QC → caption fallback
        └─────────────────────────────┼──────────────────────────────┘
                                      ▼
                    EMBED (nomic-embed-text, 768-dim) + WRITE
              one SQLite transaction per document; content-hash idempotent;
              pass cache ⇒ re-ingests only pay for what changed
                                      │
   ┌──────────────────────────────────▼───────────────────────────────────┐
   │  SQLite + sqlite-vec + FTS5                                          │
   │  L1 documents (synthesis, date, category)                            │
   │  L2 sections (summaries)          L3 chunks (raw text, provenance)   │
   │  propositions (atomic claims)     figures (VLM descriptions + PNGs)  │
   │  entities ──── entity_aliases (canonical link substrate, regenerable)│
   └──────────────────────────────────┬───────────────────────────────────┘
                                      │
                         RETRIEVE (per query, local)
   dense: propositions ─ chunks ─ sections ─ figures     lexical: FTS5/BM25
   path-anchored (code files named in query)             entity arm (alias-aware)
        → cross-encoder rerank (CPU) → diversity-aware selection
        → calibrated confidence band (flag, never filter)
        → hierarchical expansion (parent summary + doc synthesis, pure joins)
        → coarse-to-fine assembly under token budget, deduped citations
                                      │
                                      ▼
                  GENERATE: one Claude API call (multimodal —
                  top retrieved figures attach as actual images)
                                      │
              ┌───────────────────────┴───────────────────────┐
              ▼                                               ▼
        locus CLI                                  MCP server (stdio over SSH)
   query/retrieve/inspect/link/audit/eval     retrieve · list · inspect (+ opt-in query)
```

### The data model: three levels, plus first-class signal units

Every source type — a PDF, a git repository, a slide deck — reduces to the same shape:
**L1 document** (thesis/method/result/limitations synthesis, date, category) → **L2
sections** (LLM summaries; files for code) → **L3 chunks** (raw text with real provenance:
`file.py:190-218` for code, slide numbers for decks, page ranges for PDFs). Retrieval
logic never forks per format.

Two unit types are deliberately promoted out of this hierarchy and embedded directly,
because the highest-signal content must be *searchable*, not just *assembleable*:

- **Propositions** — atomic, self-contained claims extracted per section.
- **Figures** — block diagrams, plots, schematics: detected geometrically, described by a
  local VLM so they're *findable*, and attached to the Claude call as actual images so
  the precise interpretation happens in the strongest model for free.

Raw chunks are always co-assembled alongside derived units, so generation grounds on
source text even when a summary is imperfect — LLM noise is recoverable by design;
extraction loss and retrieval misses are not, and got the engineering budget accordingly.

### Math doesn't survive PDF text layers — so it's recovered

The corpus is math-dense (control theory, signal processing, quantitative finance), and
PDF text layers garble exactly that content: broken font CMaps silently *drop* ligatures,
Colab exports lose formulas entirely. A per-page damage detector (ligature-loss
signatures, symbol garbage like ω→`!`, math-font density, image-embedded formulas) routes
flagged pages through an OCR-to-markup engine.

The engine was chosen by benchmark, not reputation — three candidates raced on real
corpus pages, Claude-judged against the rendered page image: Nougat scored 0.97 *but
failed on 4/10 pages*; qwen2.5-vl scored 0.97 *but invented an equation*; **GOT-OCR-2.0
scored 0.93 with zero hallucinations and zero failures** — and was chosen on risk
asymmetry: it degrades, it never invents. Every replaced page passes deterministic QC
(length, repetition-loop, residual-corruption checks) or falls back to the original with
an audit trail.

### The link layer: entity canonicalisation as a graph substrate

"Linear, Time-invariant (LTI) models", "LTI model", "Bode Diagram"/"Bode diagram",
"fourier transform" stored as concept *and* method *and* theorem — surface variation
fragments exactly the cross-document connections a knowledge graph is for. `locus link`
builds a **derived, regenerable** alias table over the stored entities:

1. **Deterministic tiers** merge on hard evidence only: case-folding, punctuation,
   acronym↔expansion links where both surfaces are attested in the corpus, cross-document
   plurals that collapse onto an attested singular.
2. **Fuzzy lookalike clusters** (embedding cosine + token-overlap blocking) go to the
   Claude API for adjudication — judgement-quality work where a wrong merge corrupts the
   graph and a missed merge is only fragmentation.
3. **Hard guards override the model**: two names the author used distinctly *in the same
   section* never merge; short homonyms ("VaR", "P2") never merge; canonical names are
   snapped to actual corpus surfaces, never invented; code identifiers are exact and
   excluded entirely.

Verdicts are content-cached: rebuilding after new ingests re-adjudicates only changed
clusters (measured: a full re-run costs 0 API calls). The substrate powers alias-aware
entity retrieval (query "KL divergence", surface the document that only ever writes
"Kullback-Leibler (KL) divergence") and a joins-only related-documents view in the CLI
and MCP server.

### Retrieval: hybrid, reranked, diversity-aware, calibrated

Six candidate arms (four dense granularities, BM25 lexical for the exact symbols dense
embeddings blur, plus path- and entity-anchored arms) merge into a CPU cross-encoder
rerank. Selection enforces soft diversity caps — one section can't occupy three slots as
proposition+chunk+summary; one document can't monopolise the top-k and break cross-domain
synthesis; queries that name a file get the actual source file guaranteed.

Confidence is calibrated, not vibes: a rerank-score floor (fit against in-corpus queries
vs negative controls) drives a two-tier LOW CONFIDENCE banner that *flags and never
filters* — and facet-aware scoring keeps legitimate cross-domain bridge queries from
being mislabelled as absent. The labelled eval asserts banner misfires stay at zero.

### Quality engineering

The eval system is the project's spine, not an afterthought:

- **`locus audit`** — deterministic, API-free corpus QC: ingest hygiene predicates
  re-applied to stored rows, corruption signatures, unattested numbers, OCR-fallback
  counters, figure QC, alias-substrate checks (including LLM merges with zero lexical
  evidence, sampled for human review).
- **`locus eval`** — four suites: an LLM-as-judge over stored extractions (also the A/B
  harness that settled the ingest-model choice by benchmark), a math-fidelity gate that
  judges stored text against rendered page images (doubling as the VRAM-regression
  canary), a labelled retrieval suite (recall@8, MRR, cross-domain banner rate,
  file-level recall, link pairs) with answer-key exclusion — the repo indexes itself, so
  eval queries must not match their own source file — and a figure-description judge that
  compares VLM output against the actual image.
- **External adversarial audits** — desktop-Claude audits over the MCP server repeatedly
  red-teamed the system; every finding was verified against the code, fixed, and
  converted into a permanent audit predicate or eval label so it can never silently
  recur. The most instructive: a VRAM eviction bug that made math-OCR fail on 255 pages
  while *every headline gate stayed green* — the fix shipped with a new audit counter
  loud enough to catch it next time.

Hard rules throughout: every structured LLM output is schema-validated with bounded
repair retries; one bad document quarantines and the batch continues; a failed re-ingest
can never destroy the existing document; derived layers never mutate ingested data.

## Stack

| | |
|---|---|
| Extraction | PyMuPDF, python-docx, python-pptx, stdlib (md/txt/ipynb), Python `ast` |
| Math OCR | GOT-OCR-2.0 (benchmark-selected) |
| Vision | qwen2.5vl:7b via Ollama, or llama.cpp `llama-server` (Vulkan) for GPU vision encode |
| Ingest LLM | qwen2.5:7b-instruct-q5_K_M via Ollama (benchmark-selected vs llama3.1:8b) |
| Embeddings | nomic-embed-text (768-dim) |
| Store | single SQLite file: sqlite-vec (KNN) + FTS5 (BM25) + Alembic migrations |
| Rerank | ms-marco-MiniLM cross-encoder (CPU) |
| Generation | Claude API, one multimodal call per query |
| Context surface | MCP server (stdio over SSH — no open ports, no auth surface) |
| Hardware | RTX 3070 Ti (8 GB), Ryzen 5 5600X, 32 GB RAM |

## Usage

```bash
# ingest
locus ingest paper.pdf deck.pptx notes.md         # any supported format, idempotent
locus watch                                        # auto-ingest vault/incoming/<category>/
locus sync                                         # re-ingest tracked repos on new commits

# query
locus query "What did my Citadel deck recommend about monetary policy?"
locus retrieve "transfer function H(omega)" --since 2025-01-01 --category coursework
locus inspect <doc>                                # synthesis, sections, related documents

# link + quality
locus link                                         # (re)build the entity-alias substrate
locus audit                                        # deterministic corpus QC, no API
locus eval --suite full                            # judge + math + retrieval gates

# serve to Claude
locus mcp                                          # MCP over stdio (run via ssh from client)
```

Setup: `uv sync` (extras: `[rerank]`, `[mathocr]`), copy `config.example.toml` →
`config.toml`, `alembic upgrade head`, export `ANTHROPIC_API_KEY`. Requires a local
[Ollama](https://ollama.com) with `qwen2.5:7b-instruct-q5_K_M` + `nomic-embed-text`
(+ `qwen2.5vl:7b` for figures). Optional: LibreOffice (slide renders), llama.cpp
(fast figure descriptions) — absent, the pipeline degrades gracefully and says so.

## Hard-won lessons

Two findings that don't fit the decision table above because they're debugging war stories,
not choices — and they were the most expensive to learn on an 8 GB card:

- **A split model produces identical output, slowly.** The hardest bug class here: a model
  silently half-evicted to CPU passes every quality gate while running ~3× slow — or starves a
  downstream pass of VRAM entirely. Evictions are now settle-polled and confirmed, and the
  math-fidelity suite runs after any VRAM-choreography change as a regression canary.
- **Quantised vision features hallucinate.** The mmproj auto-pair at Q8_0 (vs f16) *failed* the
  figure-judge gate — the language weights quantise cleanly, the visual projector does not — a
  failure only caught because figure description quality is itself measured against the image.

## Status

Build steps 1–12 complete and gated. Next: the bulk pour of the multi-year corpus
(runbook in `docs/pour-runbook.md`), then the post-pour roadmap — ANN indexing when the
brute-force-KNN warning fires, an Obsidian projection of the canonical entity graph, and
YouTube/podcast transcript ingest.

---

*Personal infrastructure, built for one user and one server — which is exactly why the
quality bar is what it is: every document ingested badly is a document I'll never find
again.*
