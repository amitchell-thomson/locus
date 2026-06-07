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
  `ssh compute-node "cd /home/alec/server-projects/locus && /home/alec/.local/bin/uv run locus mcp"`.
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
*(Resolved 2026-06-05, post-step-10: qwen2.5:7b-instruct-q5_K_M wins — overall 3.96 vs
llama3.1:8b-q5_K_M 3.65 on the 12-section prose sample, seed 0; llama collapsed on summary
faithfulness 2.75 vs 4.50, the L2 unit. Zero schema failures for both. Incumbent stays —
no re-ingest. Harness gained prose-only sampling — code sections skip the prose passes by
design — and per-section failure tolerance counted in the aggregate.)*

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
  `{"command":"ssh","args":["compute-node","cd /…/locus && /home/alec/.local/bin/uv run locus mcp"]}`.
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
- [x] **7. Validate → re-ingest → re-evaluate** (eval phase E) — extend `locus eval` with:
  math-fidelity (fraction of math-bearing sections whose formulas survive — the gate metric),
  proposition-entailment sampling (LLM judge), labelled recall@k/MRR incl. ≥2 cross-domain
  queries; `--reingest` the 8 docs (activates `source_date`/`category`; category backfill via
  re-drop or mapping); re-run the full evaluation against the 2026-06-04 baseline.
  Done: corpus poured at 24 docs (8 rebuilds + 16 new incl. 11 quant/CS papers + CV — the
  second domain). `locus eval --suite judge|math|retrieval|full` (2026-06-05 results):
  **math fidelity 0.952** (gate PASS; text-layer baseline measured 0.73; 9/10 pages ≥0.8 —
  the one 0.75 page is a QC-fallback page by design), **retrieval recall@8 0.958 / MRR 1.000**
  (11/12 full recall; cross-domain 0.75 — the regime↔state-space query bridged domains but
  surfaced sibling quant papers over the labelled one), judge overall 3.77/5 (entity
  recall/precision weakest at 3.4/3.5 → step 12's alias resolution). Audit QC: 0 suspect
  props / 0 noise entities / 0 empty syntheses corpus-wide (was 123/209/1). The pour also
  hardened the pipeline: WAL; two-way GPU choreography (Ollama split-residency + GOT OOM);
  char-level OCR loop QC + persisted OCR audit trail in gap_flags; temperature-escalating
  repair with capped echo; per-section graceful degradation (one stubborn section costs its
  propositions, not the document). Operational rule: ONE ingest process at a time (Ollama
  contention produces spurious quarantines).
- [x] **7.5. Remediation pass 2 (2026-06-05 external evaluation)** — a desktop-Claude audit over
  the MCP server surfaced 8 issues; all fixed, one re-ingest, re-gated. **(1) LaTeX/JSON escape
  corruption** (the headline): the model wrote unescaped LaTeX into JSON strings; the parser
  turned `\tau`→TAB+`au` silently and rejected `\mu` outright → string-literal-aware sanitizer
  in `llm.py` before validation; audit predicate `has_corruption_signature` keeps it visible
  forever. **(2) Retrieval confidence**: per-citation rerank scores + doc category through
  MCP/CLI; `min_rerank_score` floor — refill never pads with below-floor candidates; best
  survivor below floor ⇒ LOW CONFIDENCE banner (flag, never filter), passed into generation.
  Calibrated on the final corpus (`scripts/calibrate_rerank_threshold.py`): weakest expected-doc
  +0.92 vs strongest negative −0.48 ⇒ **floor 0.22**; negative controls (Black-Scholes etc.)
  now flag, in-corpus queries don't. **(3) Gap-flagging inert** → evidence-grounded pass
  (section summaries + deterministic deferral-phrase hints); liveness in audit: **24/24 docs
  with ≥1 semantic gap** (was 0). **(4) Entity hygiene 2**: unbalanced-bracket reject ("SVD ("),
  ingest-time grounding (kills cross-doc bleed), `organization` type + typing few-shots.
  **(5) Sectioning**: "Figure 22:"-style caption labels rejected as headings; summary pass
  emits a semantic title replacing pagination pseudo-titles only. **(6) Propositions**:
  zero-raw output retries + logs; math-dense prompt variant (`has_math`); audit names zero-prop
  sections. **(7) De-hyphenation** at extraction (per-page, before offsets). **(8) Cross-doc
  edges** deferred to step 12 (entity-alias substrate); MCP tool docstring marks cross-domain
  bridges as the consumer's inference. Pour hardening en route: `done_reason: length` detection
  in the repair loop (demand SHORTER, not "corrected" — fixed a deterministic summary-pass
  quarantine on an OCR'd math-dense section); summaries forbid transcribing LaTeX.
  **Re-gate (24 docs, 2026-06-05)**: judge **4.08**/5 (was 3.77); recall@8 **1.000** /
  MRR 0.958 / cross-domain **1.000** (was 0.958/1.000/0.75; the MRR dip is one query whose
  expected doc is outranked by a second genuinely-relevant paper) — holds with the floor
  active; audit QC all zero + **corrupted fields 0** corpus-wide; 2 zero-prop sections
  (named, table-dense) vs the eval's method-section example now at 9 props; quarantines 0.
  Math fidelity measured **0.922** (n=20; 90% pages ≥0.8) vs 0.952 (n=8 baseline) — verified
  not a regression (de-hyphenation byte-inert on the weak pages; OCR routing code-identical;
  different sample pages), weak pages are picture-embedded formulas → step 11 scope.
  Follow-up: **doc-title arbitration** — the extractor's title heuristic stored banners
  ("ENGINEERING SCIENCE"), tab titles, slugs, fragments. The synthesis pass now also emits a
  title, used ONLY when `pdf.title_is_suspect(candidate)` (deterministic guard — an LLM asked
  to confirm a long correct title shortens it; trusted/metadata titles are never rewritten).
  Backfilled in place (`scripts/backfill_titles.py` — titles aren't embedded, no re-ingest):
  5/24 retitled, eval label synced ("mathreview" → "Probability Fundamentals"), recall held.
  **Round-2 external audit (same methodology) verified the fixes** (4 clean, LaTeX+props+
  titles+entities) and surfaced 3 actionable residuals, all fixed: **(a)** the LOW CONFIDENCE
  banner overclaimed absence on cross-domain queries → two-tier `confidence_band`
  (`ambiguous` within `DEEP_FLOOR_MARGIN=4` of the floor: "parts covered separately";
  `absent` below it) + payload pruning of sub-deep-floor noise ONLY when signal clears the
  floor (complementary facets survive; recall re-verified 1.000); **(b)** gap pass emitted
  false absences (summarisation artifacts: "does not detail θC" while text states θC=0.8) →
  hardened prompt + `filter_gaps` (keep only deferral-hint-backed or majority-unattested
  claims; hint window widened one sentence to capture pronoun-referenced topics) +
  `scripts/backfill_gaps.py` (audit lines preserved): 24-doc corpus now 13 high-precision
  gaps incl. doc 54's "root locus … for pole placement" — the exact gap eval #1 demanded;
  **(c)** de-hyphenation welded compounds ("same-\nday"→"sameday") → joins now attested
  against the doc's own vocabulary, else hyphen kept (soft-hyphen lie → space)
  `[re-ingest-bound; rides the pre-pour re-ingest]`. Remaining audit items map to existing
  steps: facet-aware scoring → §15.2 multi-query expansion; entity aliasing + cross-doc
  edges → step 12; numeric-faithfulness on propositions → §11.B/C model measurement (the
  one observed inversion is contradicted by 13 sibling props + the co-retrieved raw chunk).
- [x] **8. DOCX + Markdown/text + notebook extractors** — done (2026-06-05). Four formats:
  `.docx` (`python-docx`: heading-style sections, tables flattened pipe-joined, core-props
  title/created-date), `.md`/`.markdown` (fence-aware ATX sectioning; minimal stdlib YAML
  frontmatter → title/`source_date`; tags parsed-but-deferred), `.txt` (single section,
  char-windowed ≥12k), `.ipynb` (deferred here from step 6 — stdlib JSON, not nbconvert:
  markdown cells verbatim preserving `$...$`, code cells fenced, outputs dropped). Shared
  models + size-band machinery moved to `extract/base.py` (pdf.py re-imports; zero behavior
  change, suite-verified). Migration **0004** rebuilds `documents` to widen the source_type
  CHECK (`docx`/`markdown`/`text`/`notebook`; SQLite can't ALTER a CHECK) — applied to the
  live DB, 24 docs + children intact. Routing via `_SUFFIX_TYPE` + `_extract()` dispatch;
  pipeline downstream untouched (§2.6). **Watcher bug found & fixed during verification:**
  `_candidates` never recursed, so category-subfolder drops (`incoming/papers/…` — the
  laptop-outbox convention) sat unprocessed forever; now `rglob` with dotted-subtree
  exclusion, quarantine preserves the drop subpath. Verified live end-to-end: all four
  formats ingested + retrieved with citations (dense+lexical), frontmatter/core-props dates
  land in `source_date`, title arbitration fixed the `.txt` slug, drop-folder `category`
  derived via watcher (`notes/` → 'note'), audit QC clean; test docs then removed (corpus
  back to 24). 184 tests green (was 158).
- [x] **9. Slides (PPTX)** — done (2026-06-05). `extract/pptx.py` (`python-pptx`,
  `source_type='slides'` — semantic name per §15.4, owner decision): one span per slide
  (title placeholder + every text-bearing shape in XML order, tables pipe-joined in-flow,
  speaker notes appended under a `Notes:` marker via `notes_text_frame` — never the notes
  shapes walk, which would pull the slide-number placeholder). **Slide chrome filtered two
  ways**: slide-number/footer/date *placeholders*, and plain text boxes whose entire text
  equals the slide's own number — the first real deck (Google-Slides-style export) had NO
  placeholders at all; its page numbers were literal TEXT_BOXes, which the placeholder
  filter alone caught nothing of (found + fixed during live validation). **Sections carry REAL
  slide-number ranges in `page_start`/`page_end`** (a slide IS a page) via a private paged
  banding `_build_slide_sections` — `base.build_sections` stays unpaginated by contract;
  the banding reuses MIN/MAX + `window_by_chars` + `has_math` and threads slide ranges
  through merge (first..last absorbed slide) and windowing (parts inherit the range).
  Title/date from core properties (docx chain: core-props title >3 chars → first slide
  title → stem; created → modified → mtime). Migration **0006** widens the source_type
  CHECK (0004 rebuild pattern) — applied live, 29 docs intact. Default pass profile
  (propositions + LLM entities on). Documented limitations: shapes in XML order (no
  geometric sort — placeholder top/left inherit from layout), hidden slides included,
  images/charts contribute no text (figures = step 11). Verified live end-to-end on a
  REAL 20-slide deck (Citadel monetary-policy recommendation, dropped via LocusDrop
  `incoming/projects/`): 14 sections with honest merge ranges (pp 9–10, pp 15–20),
  speaker notes captured 1/1 (verified against the source), core-props date + drop-folder
  category landed, retrieval cites the right slides (dense+lexical), audit QC zero
  (0 suspect props / 0 noise entities / 0 corruption); chrome filter re-verified on
  re-ingest (13 bare-number chunk artifacts → 0). The deck's chart-heavy slides are the
  concrete case for step-11 figures: the empirical-model evidence lives in images the
  text layer never sees. **Deferred polish:** source-type-aware citation label ("slides 3–5" instead of
  "pp 3–5") needs `source_type` threaded through `retrieve/expand.py` → `assemble.py` →
  `mcp_server.py`; bundle with the figures/source-aware-citation work. 226 tests green
  (was 214).
- [x] **10. Code-repo ingest + continuous repo sync** — done (2026-06-05, deliberately
  ahead of step 9 by owner decision). **Two channels, one extractor** (`extract/code.py`,
  stdlib ast): (a) *tracked server repos* — config `[repos]` (all 5 under
  ~/server-projects), commit-triggered: `locus watch` checks `git rev-parse HEAD` hourly
  (`check_interval`; all-skip pass measured 0.4 s) and re-ingests only on new commits;
  manual `locus sync [--force]`; raw store = manifest JSON (git is the raw store);
  (b) *LocusDrop snapshot drops* — a directory under `incoming/projects/<name>/` ingests
  as one repo unit (ctime-settled — rsync -t preserves mtimes; tarball to vault/raw;
  stable `locusdrop:<name>` source_uri so a re-drop replaces). Doc = repo, sections =
  files, **PreChunks at def granularity with real `file_path:line` spans** (the already-
  wired §7 provenance path now lights up: live citation `analyse.py:64-102`), per-file
  `call_graph` JSON, deterministic AST entities (functions→method, classes→concept).
  **Pass profile per source_type**: code skips propositions (§11.B weakest task; chunks +
  summaries carry it, §15.0) + LLM entities; code-variant summary prompt; repo synthesis
  arbitrates titles ('digest' → 'Digest: Daily Executive News Briefing Tool'). **Pass
  cache** (migration 0005, content-keyed sha256(blob:pass:model:PROMPT_VERSION)) makes a
  commit re-ingest proportional to changed files only. **Ordering fix**: re-ingest now
  prepares first, deletes+writes in ONE transaction (previously a failed re-ingest
  destroyed the prior doc — latent `ingest_file` bug, fixed + regression-tested).
  **Ingest flock** (`vault/.ingest.lock`): manual CLI fails fast, watch skips the tick —
  the one-ingest-at-a-time rule is now a guardrail (verified live). **Live**: all 5 repos
  poured (254 sections, ~2.3k function chunks, 1.6k AST entities), audit QC clean
  (profile-aware: zero-prop code sections are by-design, not flags), retrieval cites
  file:line. Found+fixed live: `git rev-parse` walks up, so a non-git drop nested in the
  locus repo resolved to the ENCLOSING repo's HEAD → `repo_head` now requires
  toplevel == repo (regression-tested). Audit metrics mirror the pass profile via
  `pass_profile()`.
- [x] **10.5 Remediation pass 3 (2026-06-05 round-3 external audit)** — the audit's headline:
  the local model **silently hallucinates code-file summaries** (`hmm.py` → "electrical
  circuits", `evaluation.py` → "image recognition"; DB sweep found 6 more incl. `viz/tui.py` →
  "energy consumption in buildings"), with no propositions on code to catch the drift. Fixes:
  (1) **summary grounding guard** (`summarize.py`): every generated summary must share
  distinctive stemmed vocabulary with its own source (calibrated on all 766 stored sections:
  catches all hallucinations + boilerplate, 1 honest false-reject); fail → one sterner retry →
  deterministic fallback (code: docstring + def/class signature; prose: leading text), flagged
  `grounded=False` + doc gap flag; PROMPT_VERSION 2 invalidates the cached bad summaries.
  *Do this before step 11: figures route MORE non-prose through the same summarizer.*
  (2) **Facet-aware confidence** (two-rounds-standing misfire, `pipeline.py`): a bridge query
  whose units each cover one side scores below the floor on the FULL query even when both
  sides are covered — survivors are now rescored per facet (deterministic bridge-phrase split,
  `score_pairs`), and the banner is suppressed when every facet is covered (facet floor =
  floor − 2.0; fragments score systematically lower than the calibrated full questions —
  measured gap: covering units −1.0..+0.9 vs non-covering −3.9..−11.4). Verified live: the
  spectral↔regime query is now confident; negative controls still flag `absent`.
  (3) **Floor enforcement**: sub-deep-floor units are pruned whenever signal exists (full-query
  OR facet), instead of printing as co-equal results; `absent`-band keeps flag-never-filter.
  (4) **Code retrievability/noise**: implementation-intent queries guarantee a source-file
  unit in the cut (`prefer_code` in `select()`, ≥ floor only); trivial `__init__.py`
  (docstring/imports/`__all__`) skipped at extraction; `test_*` defs emit no entities (631
  of 6288 entity rows were test methods). (5) **Eval refresh** (the audit found it stale at
  24 docs and structurally blind): code+slides labelled queries incl. file-level
  `expected_paths` targeting `regimes/hmm.py`/`evaluation.py` (a summary-hallucination
  recurrence now fails the eval), and `cross_domain_banner_rate` (must be 0 — recall@k alone
  scored the misfire rounds as passing). Data: "Moentary" title typo fixed in DB; tracked
  repos force-re-ingested to regenerate summaries/entities under the guard. NOT reproduced
  from the audit: unpruned −8..−10 tails under a +5.35 top hit (current code prunes them;
  the audit also quotes the pre-7.5 single-band banner wording — stale MCP server process
  suspected). Deferred: per-slide sections (step 9 polish note stands).
  **Residual closure (same day):** (6) answer-key exclusion moved to the CANDIDATE pool —
  post-hoc scoring exclusion left self-ingested eval-file chunks consuming top-k slots
  (cross-domain recall 0.75 contaminated → 1.000 clean). (7) prior-round re-checks the
  audit handed back: gap filter HELD (0/30 docs fail re-applied predicate; fixed the new
  grounding-fallback audit lines being misclassified as semantic gaps — backfill would
  have deleted them); PCMCI/PCMIC is the PAPER'S OWN typo (attested verbatim → extraction
  faithful, aliasing stays step-12); numeric faithfulness measured corpus-wide via a new
  `unattested_numbers` audit predicate (2 real instances, both on known degraded-math
  pages with existing OCR-fallback flags; predicate knows faithful conversions: vulgar
  fractions, k-suffix, Nov-YY years, decimal commas, digit lists vs thousands-grouping).
  (8) source under-retrieval CLOSED: path-anchored search arm (file stem named in query →
  section candidate with grounded signature summary) + query-named exemption in select()
  (per-doc cap + child-redundancy demotion are breadth rules; neither applies to a file
  the query names — a repo is one doc but many files). Final re-gate: recall@8 **1.000**,
  full-recall 1.000, cross-domain 1.000, banner rate 0.000, **file_recall 1.000** (was
  0.333). All round-3 findings and hand-backs closed; figures (step 11) unblocked.
- [x] **11. Figures** `[RB]` (medium) — **landed + corpus re-ingested 2026-06-06** (28/28
  docs in 5.18 h, zero quarantines; **389 figures, 100% described + vectored**; categories
  intact — coursework 12 / paper 12 / project 8 / career 1, zero uncategorized). Re-gate
  vs step-7.5: recall@8 **1.000**, cross-domain 1.000, file_recall 1.000, banner rate 0,
  judge 4.02 (baseline 4.08, n=8 noise; entity recall still weakest → step 12), audit QC
  zero on every re-ingested doc (the only noise entities live on two code repos untouched
  by this batch → step-12 hygiene). End-to-end verified live: MCP `retrieve` returns
  labeled ImageContent blocks; `locus query` answered a block-diagram question from 3
  attached figure images with `[figure on p.N]` citations; the Citadel deck's 11 chart
  slides are searchable (the step-9 gap, closed). Fixes landed during the run:
  **re-ingest metadata continuity** (raw-store re-ingest inherits category/source_uri from
  the replaced doc — caught live when a test re-ingest wiped a 'paper' to 'uncategorized');
  slide-caption fallback to first body line (placeholder-less decks); eval-label updates
  for corpus growth ('|' any-of in expected entries; slug-titled docs re-arbitrate titles
  per re-ingest so labels keep durable tokens). Driver: scripts/reingest_step11.py.
  Schema: migration 0007 `figures` + `figure_vectors` vec0 (propositions mirror; nullable
  `section_id`; PNGs in the flat raw store as `{hash}_fig{N}.png`, orphan-cleaned on
  replace/delete). **Tier 1 (preserve):** `extract/figures_detect.py` — raster
  (`get_image_info`, xref-deduped doc-wide) + vector diagrams (`cluster_drawings` ≥8 paths;
  the control-notes block-diagram case), clip-rendered at 2x (handles masks/composites;
  vector clusters have no xref), area band 3–92% of page, aspect ≤20:1, formula box shared
  with the math detector via `base.SMALL_IMAGE_MAX_*`. Caption pairing (below > above >
  contained — vector clusters swallow their own captions) decides survival: figure-class
  caption = author's assertion (overrides density; recovers text-boxy flowcharts at 5–7
  chars/1000pt²), table-class caption = reject (text layer already has it), uncaptioned =
  keep only under `max_text_density` 2.0 (corpus-measured: real diagrams ≤1.6,
  prose-with-formula-drawings/gridline tables ≥2.2 — the doc-50 Colab false-positive
  class). Corpus sweep after calibration: 347 figures, 72% captioned, 0 junk on prose
  docs. Slides: visual-bearing slides (chart/SmartArt/media/picture ≥3% slide area — the
  0.26% template logo dies; drawn-autoshape census deliberately NOT a signal, consulting
  decks build text layouts from large autoshapes) render whole via `soffice --headless` →
  PDF → PNG; soffice absent/failing/page-count-mismatch degrades to a `figure_fallbacks`
  gap line, never quarantines. **Tier 2 (findable):** `ingest/figures.py` VLM pass
  (qwen2.5vl:7b via the new shared `llm.vision_chat`, refactored out of mathocr's qwen
  engine), faithfulness prompt + deterministic QC (empty/short/refusal/repetition-loop) +
  one bounded retry → caption-only fallback; batched AFTER all text passes (one
  Ollama model swap per doc); `caption+description` embedded into `figure_vectors`;
  `_FigureCache` keyed sha256(image):model:PROMPT_VERSION ⇒ re-ingests re-run ZERO VLM
  calls. **Retrieval:** 4th dense arm (`figure_top_k`), reranker scores the indexed text,
  `select()` caps per (section, figure) and figure-children demote section summaries;
  expansion carries figure provenance (figure page beats section range); citations read
  `[figure on p.N]`/`[slide N]`. **Tier 3 (interpretable):** `RetrievalResult.figures`
  (top `image_cap`=3 by rerank) → `query.py` attaches the actual PNGs as base64 image
  blocks in the uncached user turn (cached system prefix unaffected) with downscale
  guard (1568px/3MB, `retrieve/figure_images.py`); MCP `retrieve` returns
  `[text, *labels, *Image]` content blocks (gated `mcp.include_figure_images`); missing
  image always degrades to text-only. Audit: per-doc figures line (caption-only /
  unsearchable / suspect descriptions re-applying the QC predicate); figure audit lines
  excluded from semantic-gap counts. Eval: 3 figure-shaped labelled queries. Verified
  live: doc 91's "Figure 2" pipeline diagram is the TOP survivor (rr +8.49) for its
  figure query, beating the caption-bearing chunk. **Re the 10.5 directive ("figure
  text MUST go through the summary grounding guard"): resolved as not-applicable by
  design** — figures never pass through the summarizer (captions are extracted verbatim,
  deterministically), and the VLM description's source is the IMAGE, not section text,
  so vocabulary-grounding has nothing to ground against; the hallucination cost is
  bounded instead by the faithfulness prompt + QC, the caption given as anchor context,
  and tier 3 (Claude sees the actual image at generation and can override a wrong
  description — §15.0 recoverable class). System dep: **LibreOffice** (`soffice`) for
  slide renders, optional at runtime. Deferred: PPT-drawn box-and-arrow diagrams
  (autoshape census misfires on card-layout decks; box text still extracted via the text
  layer), per-figure regions on slides (whole-slide render is the unit).
- [x] **11.5 Remediation pass 4 (2026-06-06 external figure audit)** — a desktop-Claude audit
  of the step-11 figure layer (uniquely able to judge VLM descriptions against the attached
  images) found 7 issues; verification against the DB showed its HIGH finding far larger than
  sampled: **the GOT math-OCR pass had OOM'd on 255 pages across 14 docs during the step-11
  re-ingest** — the new figures VLM was the last GPU user of every figure-bearing doc, and
  `ocr_pages` evicted only the ingest model. One bug class, four manifestations, each now
  guarded: (1) evict ALL resident models before GOT (`llm.unload_all`); (2) eviction is
  CONFIRMED, not issued — Ollama delists a model before its runner frees VRAM, so a consumer
  loading in that window OOMs/splits (settle-poll in `unload_all`); (3) qwen2.5vl at
  num_ctx=8192 does NOT fit the 8 GB card — Ollama plans a KEPT 74/26 CPU/GPU split (the
  "GPU idle, llama-server burning 6 cores" signature; figure phases ran ~3x slow through the
  whole step-11 batch) — descriptions need ~no context, so `num_ctx=4096` (verified 100%
  GPU, model 5.7→4.4 GB); (4) the same teardown race at the text→VLM handoff (`llm.unload`
  settle-polls too; landed after the recovery run — validates on the next VLM-heavy run,
  e.g. the fig-v2 backfill). Plus: per-page OOM retry after a 15 s teardown wait; engine
  errors store one truncated line (no tracebacks/PIDs in gap_flags); audit gains an
  **OCR-fallback page counter** + heavy-fallback warning (the regression was invisible to
  every headline gate — and the math suite, skipped at the step-11 re-gate as "extraction
  unchanged", would have caught it: run it whenever VRAM choreography changes); math eval
  fixed to sample PDFs only (crashed on repo manifests). Other audit findings: fig-v2 prompt
  (diagram topology + blur honesty; structural hallucinations gone on the audit's repro,
  residual terminology slips remain — §11.B class, bounded by tier 3); image-attachment
  floor (min_rerank_score gates IMAGES only; text/citations stay flag-never-filter); cap
  notes on MCP + query ("N figures cited; K attached") so citations never dangle. Recovery:
  targeted re-ingest selected by OOM gap signature (97→98 pages, two passes — the first
  re-lost 31 pages to manifestation (2) before it was understood), final state **zero OOM
  gaps corpus-wide**, 19 fallbacks all QC-reasoned, categories intact, 389 figures intact.
  Re-gate: math fidelity 0.876 (n=20; baseline 0.922 — verified sample-composition + the
  known picture-embedded-formula ceiling, NOT recovery damage: the weak pages were
  successfully-OCR'd hard pages, and the one QC-fallback page in the sample scored 0.85).
  Residuals: fig-v1 descriptions on ~223 figures (cheap backfill before the pour — also the
  validation run for guard 4); LibreOffice font substitution overlaps slide renders (host
  fonts); diagram terminology slips (§11.B). **Operational lesson: a split model produces
  identical outputs SLOWLY — no gate catches it; watch for GPU-idle + multi-core
  llama-server during ingest.**
- [x] **11.6 llama.cpp GPU vision backend for figures** — **done** (2026-06-06, same
  evening). Ollama ≤0.30.6 runs the qwen2.5vl vision encoder on CPU (`clip_ctx: CLIP
  using CPU backend`, unchanged after upgrading 0.30.2→0.30.6) — ~27.5 s/figure measured
  across the fig-v2 backfill. Fix: serve the SAME weights via `llama-server` (official
  **Vulkan** Ubuntu build b9544 at `~/.local/opt/llama-b9544/` — no official Linux CUDA
  binary exists; Vulkan offloads LLM + mmproj to the 3070 Ti) with
  `ggml-org/Qwen2.5-VL-7B-Instruct-GGUF:Q4_K_M` (~4.7 GB HF download; Ollama's blobs are
  owner-unreadable and qwen mmproj blobs are cross-incompatible). Implementation:
  `[figures] engine = "ollama"|"llamacpp"` (+ `llamacpp_*` keys); `ingest/llamacpp.py`
  `LlamaServer` context manager (spawn → `/health` poll → terminate; `unload_all`
  confirmed-settle owns the card handoff); OpenAI data-URI chat; `describe_figure`
  dispatches on a `.proc`-bearing client — QC/retry/downscale unchanged;
  `_describe_figures` in the pipeline spawns PER-DOC-BATCH (a long-lived server would
  starve the text passes — the split trap) and **fails closed to ollama** with a doc gap
  on any lifecycle failure (binary absent, startup timeout, mid-batch death — remaining
  figures redone via ollama; never quarantines); `_FigureCache` keys on the EFFECTIVE
  engine model string (ollama keys untouched; engines never reuse each other's outputs).
  **Judge gate (eval/figure_fidelity.py + scripts/judge_figures.py, 10 figures vs the
  stored backfill baseline, Claude judging against the actual images): the Q8_0 mmproj
  auto-pair FAILED (faith 2.80 vs 3.10, halluc 24 vs 18 — quantized visual features
  invent more); the f16 mmproj PASSED — faith 3.10 vs 3.00, concreteness 3.60 vs 3.40,
  halluc 20=20, zero failures, 2.1 s/figure (13x).** Engine flipped in live config with
  the explicit f16 `llamacpp_mmproj`. Verified end-to-end: a re-ingest under the engine
  produced clean figures with no fallback gap; audit QC zero; math suite 0.889 (in band —
  the run-after-any-VRAM-change rule). Pour figure economics: ~35 min per 1000 figures
  (was ~7.5 h). 11 model-free tests (lifecycle/request-shape/dispatch/fallback).
  System dep: the llama.cpp binary (optional, like soffice — absent ⇒ ollama path).
  Residual: figure-description quality itself (~3 faithfulness on hard diagrams) is the
  §11.B model ceiling, unchanged by the executor — revisit at the next VLM generation.
- [x] **12. Entity-alias resolution + Retrieval eval (Layer 3)** — done (2026-06-06); the
  **BULK INGEST itself remains** (owner-run per `docs/pour-runbook.md`; pre-pour blocker:
  the laptop outbox's `coursework/` folder is not syncing). Canonicalises entity names
  cross-document — the *link* substrate. **Substrate:** `entity_aliases` (migration 0008) —
  DERIVED + REGENERABLE total mapping `(name,type) → (canonical_name, canonical_type)`;
  `entities` never mutated; rebuild = delete + recompute; built by **`locus link`**
  (`locus/link/aliases.py`). Tiers: deterministic first (casefold / punct / attested
  acronym-expansion incl. plural-chained lookups ("LTI models"→attested "LTI model") /
  attested cross-doc plural — same-type only, hard evidence), then embedding-blocked
  lookalike clusters (cosine ≥0.86 AND token-Jaccard ≥0.34 — the token guard kills
  theme-mates like Kalman/particle filter) adjudicated by the **Claude API** (owner
  decision; forced tool-use, judge.py pattern), verdicts cached in pass_cache keyed on
  cluster content+model+PROMPT_VERSION ⇒ incremental re-runs are ~free (verified: rerun =
  359 cache hits, 0 API calls, byte-identical). Hard guards override the LLM (it proposes,
  aliases.py disposes): min-merge-length 4 (homonyms: 'var', 'P2'); same-section
  co-occurrence ⇒ never merge (authorial evidence); canonical snapped to an actual member
  surface; code docs excluded from clustering (AST identifiers exact; identity rows keep
  the join total); oversize components (>8 reps) skip the LLM (cost guard — correctly
  skipped the MSH-variant and Qwen-model-zoo families). **Live build (33 docs):** 4,696
  identities → 4,257 clusters (340 non-trivial, 779 variants merged; llm 502, casefold
  165, punct 54, plural 33, acronym 25); cross-doc canonicals 182→**211**; 17 guard
  splits; audit suspicious-merges (llm-tier, zero lexical evidence) **0**; spot-checked
  sample clean (type collapses, author normalisation, no wrong merges). **Consumers:**
  entity-anchored retrieval arm matches via canonical groups (query "KL divergence"
  surfaces the doc storing only "Kullback-Leibler (KL) divergence"; substrate checked per
  query, no process cache ⇒ MCP picks up rebuilds without restart; empty-table fallback =
  pre-step-12 behaviour); `related_documents` (`locus/link/related.py`, joins-only) in
  `locus inspect` + MCP `inspect_document` — the step-7.5 cross-doc edges, with a
  stop-entity guard designed now and OFF until the pour (~0.4×doc count). **Eval Layer 3
  (pre-pour gate, 33-doc heterogeneous corpus incl. quant papers + 5 code repos):**
  recall@8 **1.000** / cross-domain **1.000** / banner **0.000** / file_recall **1.000**
  (21 labelled queries incl. a new alias-bridged one) + **links_recall 1.000**
  (`score_links`, 4/4 labelled related-pair directions); judge **4.35** (n=8; baseline
  band 4.02–4.08; entity recall still weakest 3.38 — extraction recall is the §11.B model
  ceiling, untouched by aliasing); math fidelity **0.928**, 87.5% pages ≥0.8 (in the
  measured band). Audit gains the ALIAS SUBSTRATE QC block; config `[alias]`. 322 tests
  green (27 new). The fig-v1→v2 backfill + VLM placement pre-pour items were closed by
  11.6.

**After the pour (not `[RB]`):** ANN-index warning (§11.D), Obsidian projection (§14),
YouTube / podcast transcript ingest, broader retrieval tests.

**New dependencies:** `mcp`, `python-docx` (landed, step 8), `python-pptx` (landed, step 9),
an OCR-to-markup
model (benchmarked, step 6), `pillow` promoted to core (step 11: generation/MCP image
downscale; was mathocr-extra-only). `nbconvert` turned out unneeded — the `.ipynb` extractor
reads the notebook JSON with the stdlib (§3 build-vs-buy). Keep heavy optional ones (VLM,
math-OCR) behind extras like `[rerank]`. **System dep (optional):** LibreOffice (`soffice`)
for step-11 slide figure renders — absent, slide decks ingest text-only with an audit gap line.
