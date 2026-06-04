# Locus — Development Plan

> Human-readable roadmap. The authoritative goal/architecture is in `CLAUDE.md` (§1, §15, §16);
> this file is the working plan and is kept in sync with `CLAUDE.md §16`.

## Goal (CLAUDE.md §1)

A self-hosted system to **query, link, and serve as Claude's context** over the owner's entire
personal knowledge base — technical reading *and* projects, achievements, history, general info.
Three equal uses: **query** (grounded cited answers), **link** (connections across the corpus),
**feed Claude** (the KB as on-demand context inside Claude).

## Status — critical path complete

Ingest → hybrid retrieval → grounded answer works end-to-end on the current corpus (8 docs).

- **Done:** scaffolding/config/`.env`; Alembic + sqlite-vec + FTS5 DB; PDF extraction (robust
  sectioning); chunk + embed (nomic, 768-dim); local-LLM passes (qwen2.5:7b-instruct-q5);
  ingest orchestration (idempotent, transactional, `--reingest`); eval harness (structural audit
  + Claude LLM-judge); hybrid retrieval (dense + FTS5/BM25 + entity → cross-encoder rerank →
  expand → assemble); query (single Claude call, modes); folder watcher; laptop→server outbox.
- **CLI:** `ingest · watch · list · inspect · audit · eval · retrieve · query`.

---

## MCP architecture — local Claude ↔ server-side corpus

**Yes, it works.** The MCP *server* runs **on the server** (it needs the SQLite DB, sqlite-vec,
Ollama for query embedding, and the cross-encoder reranker). Your local Claude (Claude Code CLI /
desktop app) is the *client*. The transport bridges them:

- **stdio over SSH (recommended).** Configure the MCP server as a local command that is really an
  SSH into the server, e.g.
  `ssh locus-server "cd /home/alec/server-projects/locus && uv run locus mcp"`.
  Claude spawns `ssh` locally; the Locus MCP server runs remotely; stdio is tunneled over the SSH
  connection you already use for the outbox. **No open ports, no extra auth, no new infra.** The
  server process stays alive for the session, so the embedder + reranker load once and stay warm.
- **HTTP (Streamable HTTP / SSE).** Run the MCP server as a network service and reach it via an
  SSH tunnel (`ssh -L`), Tailscale, or a port-forward; point Claude at the URL. More setup
  (networking + auth); use only if you want a persistent shared service.

**Tool design:** expose **`retrieve`** (returns assembled context + citations) as the core tool —
your local Claude pulls your KB as context and does the generation itself. This is the natural MCP
pattern and needs only the DB + Ollama + reranker (no server-side Claude key). Optionally also
expose `query` (server-side generation), `list`, and `inspect`.

---

## 2026-06-04 corpus evaluation — what it found, what it changes

A full external evaluation (Claude over the MCP server, probing ingest artefacts + live
retrieval on the 8-doc corpus) surfaced defects that re-order the plan. Verified against the
code (not just the eval's claims):

- **Math destroyed at extraction** (live): broken font CMaps *drop* ligatures (`rst-order`,
  `dened`) — unrecoverable post-hoc, no character map can restore them; Colab-export PDFs lose
  formulas entirely. Corruption also defeats the `has_math` regex (52/109 sections flagged in a
  PDE doc), so the planned math-OCR pass would miss its targets. → phase **6** below.
- **Sectioning over-fragments** (live, current extractor reproduces it): the font heuristic
  accepts full-sentence outline lines as headings → 109 sections / 78 pp, mid-sentence titles;
  ToC/dotted-leader pages ingest as content and pollute top-k. → phase **4**.
- **Pass hygiene**: all-empty doc synthesis is schema-valid so it ships; tautological/meta
  propositions; entity noise (equation labels, bare symbols, surface-form variants). → phase **5**.
- **Retrieval selection**: candidates dedupe by (kind, id) only, so one section occupies up to
  three rerank slots (proposition+chunk+section); pure-relevance top-8 collapses onto one
  document, which breaks cross-domain synthesis — the headline capability. → phase **3**.
- **Date/category facets dead on the current corpus**: *already fixed in code* (migration 0003
  post-dates the ingest); a `--reingest` activates them. → phase **7**.

**Sequencing consequence:** retrieval selection first (cheap, not RB, improves daily MCP use);
then all re-ingest-bound fixes (sectioning, pass hygiene, math) **before** the one re-ingest of
the small corpus; then re-evaluate against the 2026-06-04 baseline; then resume breadth.
The §11.C model benchmark (llama vs qwen) stays deferred until extraction is fixed — both
models currently summarise math-stripped text, so it would measure nothing.

## Ordered plan

`[RB]` = re-ingest-bound: changing it later forces re-ingesting the whole corpus, so it must be
done **before** the bulk ingest. Each item ≈ one work-block.

- [x] **1. Temporal + category metadata** `[RB]` — `documents.source_date` + `category`
  (PDF metadata date → file mtime; category by drop-folder/heuristic); retrieval facets
  (`--since`/`--until`/`--category`); `audit` shows the distribution. *Schema lock — first.*
  Done: migration 0003; `extract/pdf.py` parses the PDF creation/mod date; `ingest_pipeline`
  applies the mtime fallback + derives category from the drop folder; `Facets` filter in
  `retrieve/search.py`, wired through `retrieve`/`query` CLI. Existing pre-0003 docs read NULL
  (excluded by date facets) until a `--reingest` repopulates them.
- [x] **2. MCP server** — `retrieve`/`query`/`list_documents`/`inspect_document` as MCP tools
  (stdio over SSH). Daily utility over the current corpus; not `[RB]`. **Primary deliverable.**
  Done: `locus/mcp_server.py` (FastMCP); `locus mcp` runs it over stdio. Tools accept the
  `since`/`until`/`category` facets. `retrieve` needs no Claude key (client generates); `query`
  generates server-side and is **opt-in** (config `[mcp].enable_query` / `--enable-query`, default
  off) so the billable tool isn't advertised unless asked. Verified end-to-end over a real stdio
  client handshake. Client config:
  `{"command":"ssh","args":["locus-server","cd /…/locus && uv run locus mcp"]}`.
- [x] **3. Retrieval selection (eval phase A)** — *not RB; do first.* In `retrieve/rerank.py`:
  after cross-encoder scoring, cap units per `(section_id, kind)` at 1 and demote
  section-summary candidates already represented by a child (expansion re-attaches the summary
  anyway); per-doc diversity cap (≤3 of top-8, config `per_doc_cap`) with fallback fill so the
  top-k stays full — fixes single-doc collapse on synthesis queries. Dedupe citations in
  `assemble.py`. Verify on the Biot (no dup sections) + Fourier-bridge (≥2 docs) queries.
  Done: `select()` in rerank.py (soft caps, refill, rank order restored); verified live —
  Biot citations unique, Fourier bridge surfaces 3 documents (was 1).
- [x] **4. Sectioning + front-matter** `[RB]` (eval phase B) — in `extract/pdf.py`, deterministic:
  detect ToC/front-matter (dotted-leader density, pre-first-heading position) and exclude it from
  passes + embedding; title-shape filters on heading candidates (reject mid-sentence/full-sentence
  lines); raise `MIN_SECTION_CHARS` ~1.5–2k (measure the distribution across the 8 docs first);
  tighten the section-count sanity cap (~`max(8, pages/2)`). Unit-test against the 8 raw PDFs.
  Done, with two measured deviations: (i) printed-ToC pages are *excised at extraction*
  (page blanked, `ExtractedDoc.toc_pages` audit trail) instead of an `is_frontmatter` tag —
  stronger, no schema change; (ii) `MIN_SECTION_CHARS` stays 400 — after `_plausible_heading`
  shape filters (lowercase/punct start, >8 words, multi-sentence, equation glyphs, unbalanced
  parens, control chars, no real word) the fragmentation vanished and remaining small sections
  are real titled subsections (PDE doc: 109→37 sections, median 745→2540 chars; all 8 docs
  clean per `scripts/measure_sectioning.py`; Optimization doc recovered real headings). Sanity
  cap tightened to `max(8, 1.5×pages)` post-filter. Effective only on re-ingest (step 7).
- [x] **5. Pass hygiene** `[RB]` (eval phase C) — proposition prompt forbids meta-statements +
  deterministic post-filter (meta regexes, near-dupe-of-title, dropped-formula signatures), one
  bounded retry; *semantic* synthesis validation (all-empty = failure → repair → quarantine, not
  silent ship); entity surface normalization at write (case/plural/punctuation) + filters for
  equation labels and bare symbols (full cross-doc alias resolution stays in step 12); `audit`
  gains a QC section (filtered-proposition counts, empty syntheses, corruption rate).
  Done: `rejection_reason`/`filter_propositions` (fragment / too-short / meta / dropped-formula /
  title-echo) + retry-once-if-all-rejected in `propositions.py`; `DocSynthesis` field validators
  reject blank fields → llm.py repair loop → quarantine; `normalize_name`/`is_noise`/
  evidence-based `merge_plural_variants` in `entities.py` (plural collapses only onto an attested
  singular — "Fourier series" is never mangled); audit re-applies the same predicates to stored
  rows. Verified on the live (pre-re-ingest) corpus: QC finds doc 19's empty synthesis, 123
  suspect props (incl. the Colab "given by ." pair), 209 noise entities — the step-7 re-ingest's
  cleanup, quantified. Effective on re-ingest.
- [x] **6. Math-faithful extraction** `[RB]` (eval phase D; was step 7, promoted) — corruption
  detector first (ligature-loss signatures, math-font evidence from span fonts → also fixes
  `has_math`); route corrupted/math-dense pages to an OCR-to-markup model chosen *empirically*
  on ~10 flagged corpus pages (Nougat-small / texify / Marker / GOT class — don't pick by
  reputation); native `.ipynb` extractor (`nbconvert` → md, preserves `$...$` + code cells) and
  prefer source formats over PDF exports. Then (and only then) run the §11.C model benchmark.
  Done (`.ipynb` deferred to step 8 by owner decision): **(a) detector** — per-page `PageFlags`
  in `extract/pdf.py`, four corpus-verified signals (word-boundary broken-ligature words;
  `ω→'!'` symbol garbage; TeX-math-font / math-unicode density; image/vector-drawing formulas
  with a zero-false-positive inline-gap shape); `has_math` now uses font/image evidence; 238/436
  corpus pages flagged. **(b) benchmark** — `scripts/benchmark_mathocr.py` raced qwen2.5-vl:7b
  (Ollama), GOT-OCR-2.0 and Nougat-small on 10 flagged pages; `scripts/judge_mathocr.py` +
  `locus/eval/math_fidelity.py` (Claude multimodal judge, doubles as the step-7 gate metric)
  scored them against the text-layer baseline: text-layer **0.73**, GOT **0.93** (0 halluc,
  0 failures, prose 4.9/5, ~10s/pp), Nougat 0.97 *but 4/10 engine failures*, qwen 0.97 *but 1
  invented equation*. **GOT-OCR-2.0 chosen on risk asymmetry** (degrades, never invents) —
  report: `eval-artifacts/mathocr/report.md`. **(c) routing** — `extract/mathocr.py`: flagged
  pages re-read by the engine, whole-page replace guarded by deterministic QC (empty/too-short/
  repetition-loop/residual-corruption → keep original + audit trail); `extract_pdf(mathocr=True)`
  used by the pipeline only; config `[mathocr]` (`engine = "got" | "qwen" | "off"`); deps behind
  the `mathocr` extra. Verified live on doc 24 (mangled `𝑐(𝑤)` → `\\(c(w): \\mathbb{R}^d \\to
  \\mathbb{R}\\)`). §11.C ingest-model benchmark is now unblocked. Effective on re-ingest.
- [ ] **7. Validate → re-ingest → re-evaluate** (eval phase E) — extend `locus eval` with:
  math-fidelity (fraction of math-bearing sections whose formulas survive — the gate metric),
  proposition-entailment sampling (LLM judge), labelled recall@k/MRR incl. ≥2 cross-domain
  queries; `--reingest` the 8 docs (activates `source_date`/`category`; category backfill via
  re-drop or mapping); re-run the full evaluation against the 2026-06-04 baseline.
- [ ] **8. DOCX + Markdown/text extractors** — `python-docx` + `.md`/`.txt`; route in watcher.
- [ ] **9. Slides (PPTX)** — `python-pptx`: per-slide text + speaker notes; images feed figures.
- [ ] **10. Code-repo ingest** — `python-ast` (functions, call graph, per-file sections);
  repo-directory entry point; `file:line` provenance already wired.
- [ ] **11. Figures** `[RB]` (medium) — `figures` + `figure_vectors`; extract/store/caption +
  optional local VLM description; multimodal Claude at generation.
- [ ] **12. Entity-alias resolution + Retrieval eval (Layer 3) → BULK INGEST** — canonicalise
  entity names cross-document (the *link* substrate); validate on a heterogeneous sample
  including quant-finance papers + ≥1 code repo (the second domain the synthesis modes need);
  then pour the corpus.

**After the pour (not `[RB]`):** ANN-index warning (§11.D), Obsidian projection (§14),
YouTube / podcast transcript ingest, broader retrieval tests.

**New dependencies:** `mcp`, `python-docx`, `python-pptx`, `nbconvert`, an OCR-to-markup model
(benchmarked, step 6). Keep heavy optional ones (VLM, math-OCR) behind extras like `[rerank]`.
