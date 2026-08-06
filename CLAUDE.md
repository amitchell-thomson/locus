# CLAUDE.md — Locus

Working context for Claude Code. Authoritative for architecture, invariants and conventions.
Where this conflicts with an ad-hoc instruction in chat, ask before deviating.

Development narrative — who found what, when — lives in git history and commit messages. This
file is orientation, not a changelog. What appears here is what you need in order to change the
code without breaking something invisible.

---

## 1. What it is

A self-hosted system for querying, linking and serving as Claude's context over the owner's
personal corpus: papers, lecture notes, code repos, slide decks, project write-ups, handwriting.

Three co-equal uses:
1. **Query** — grounded, cited answers over the corpus.
2. **Link** — connections *across* it (cross-domain, cross-project, cross-time).
3. **Context for Claude** — retrieval backend, live via the MCP server.

**Design objective:** maximise retrieval answer quality per query. Ingest cost is unbounded;
retrieval latency and answer quality are the only constraints that matter. Workflow is
query-driven and page-driven, never browse-driven. No GUI.

**Current scale (2026-08-06):** 232 documents · 3,102 sections · 11,961 chunks · 16,975
propositions · 24,778 entities · 2,422 figures · 2,216 cross-doc canonicals · 208 MB ·
~1,057 tests. Corpus by category: coursework 144 · note 32 · project 22 · paper 18 · career 16.

---

## 2. Invariants — do not break these

These are the rules that, when violated, fail *silently*. Most were learned by shipping the
violation.

1. **Grounded or silent.** Every proposition, connection, tension, answer, critique and practice
   item cites a real stored unit, or it does not appear. A model told to cite will cite,
   including keys it invented — so citation is *verified after the call*, never trusted.
2. **Propose, never mutate.** The agent layer writes to its own tables. `upsert_object` never
   writes `status`; bodies merge additively. The ingested corpus is never edited by a derived
   layer.
3. **Derived data is regenerable.** Aliases, pass caches, Obsidian exports, gate logs: rebuild =
   delete + recompute. Ingested tables are the only source of truth.
4. **Idempotent ingest by content hash.** Re-ingest of identical content is a no-op. Prepare
   first, then delete+write in one transaction — a failed re-ingest never destroys the prior doc.
5. **Agent prose must never re-enter the corpus as his words.** Only fields carrying an
   `_owner_edits` marker are promoted to `vault/notes/`. `_generated/` is corpus-excluded and
   never read back by the structurer.
6. **One decision, one surface.** A pending item appears in `locus decide` *or* on the daily
   page, never both. `decide/queue.pending` subtracts what `compose_daily` is offering; a test
   asserts the key sets never intersect.
7. **The daily page is aggregate-only.** Composition makes no model call. Every piece of prose it
   prints was written and stored by an earlier pass, so the page renders even when last night
   failed.
8. **One section per physical page.** Layout is a derived constraint. Change `[daily].rule_gap_em`
   and the line budget must be re-measured by rendering a real PDF and counting pages.
9. **Nothing is shown twice.** `daily_shown` retires an item by `item_key`, which carries the
   item's *version* (`object:41:<updated_at>`) — so a developed thread returns and an untouched
   one does not.
10. **One ingest process at a time**, enforced by an advisory flock. Concurrent Ollama access
    produces spurious quarantines.

---

## 3. The failure class this codebase is built to resist

**A path that looks wired and isn't — failing silently, with tests passing either side.**

It has occurred repeatedly and in the same shape. Representative instances, all real:

| What looked fine | What was actually true |
| --- | --- |
| Tension detection, "the headline capability" | A distance filter of 0.55 admitted only paraphrases of the owner's own view. 16 positions, 0 tensions, for its entire life. |
| The re-read slot on the Read page | Rejected 190 of 190 candidates; best score ever 1.917 against a floor of 2.5. Never fired once. |
| A cached "no tension" verdict | Re-judged only when a *document* arrived — silent about the prompt. An improved judge would have shipped and changed nothing. |
| Model-supplied citation keys | The key resolved, so the check passed; the passage was irrelevant. Checking existence catches invented keys, never wrong ones. |
| `channel_stats` per-channel breakdown | Grouped by a column that held one constant value. Could never produce more than one bucket. |
| Highlight capture | Cluster gap 26pt vs 12pt line spacing merged every highlight on a page into one mark. |

**Consequences for how you work here:**

- A test fixture is built to clear the gate, so tests cannot detect a gate that admits nothing.
  Reading the code cannot either — the constant looks reasonable.
- **`locus gates`** (`locus/observe/gates.py`) exists for exactly this: it records what each
  threshold *rejected*, aggregated per gate per day with verbatim samples, and calls out in words
  any gate with a 100% reject rate. Instrument a new threshold when you add one.
- **Verify against real output**, not against the tests. Several conclusions in this project's
  history were wrong because verification ran against the wrong checkout or a stale config.
- When you set a threshold, prefer **recording the distribution first** over guessing a number.

---

## 4. Hardware envelope (binding)

| Resource | Spec | Consequence |
| --- | --- | --- |
| GPU | RTX 3070 Ti, **8 GB VRAM** | The architectural driver: local models ≤ ~8B quantised, ingest local, generation hosted, strict VRAM choreography |
| CPU | Ryzen 5 5600X | Cross-encoder reranking runs here |
| RAM | 32 GB | SQLite + Ollama overhead |
| Storage | 1 TB SSD | Flat raw store + one SQLite file |

**VRAM choreography (hard-won):** evictions must be settle-polled (Ollama delists before VRAM
frees); evict all models before GOT-OCR; `qwen2.5vl` needs `num_ctx=4096` to fit. A split model
produces *identical output slowly* — no quality gate sees it. **Run the math eval suite after any
VRAM change.**

---

## 5. Data model

Source of truth: Alembic migrations in `locus/db/migrations/versions/` (currently **0034**).
`locus/db/schema.sql` is a human-readable summary. Never `ALTER` a live DB ad hoc; never force a
re-ingest for a schema change — migrate forward.

### Corpus (the spine — immutable to derived layers)

- **documents** (L1) — `content_hash` UNIQUE, `source_type`, `source_uri`, `raw_path`, `title`,
  `source_date`, `category` (paper|coursework|project|career|note), discrete synthesis columns
  (`thesis`/`method`/`result`/`limitations`), `section_map`, `gap_flags`, `maturity`,
  `structured_at`.
- **sections** (L2) — position, title, LLM summary, `file_path` + `call_graph` for code.
  + `section_vectors`.
- **chunks** (L3) — ~512-token raw text with provenance (`file:line`, slide number).
  + `chunk_vectors` + `chunks_fts` (FTS5, trigger-synced).
- **propositions** — atomic self-contained claims, first-class and embedded. The highest-signal
  unit must be directly searchable.
- **figures** — caption + VLM description, embedded; PNGs in the raw store, orphan-cleaned.
- **entities** — typed, section-anchored, `UNIQUE(doc_id, section_id, name, type)`.
- **entity_aliases** — DERIVED total map `(name,type) → (canonical_name, canonical_type)` +
  cluster + tier. Built by `locus link`; `entities` is never mutated; singletons map to self so
  consumers can inner-join.
- **pass_cache** — content-keyed LLM outputs, so re-runs pay only for what changed.

### Agent state (never touches the spine)

`objects` · `object_links` · `belief_positions` · `belief_tensions` · `review_schedule` ·
`mark_answers` · `connection_notes` · `acceptance_log` · `agent_runs` · `daily_pages` ·
`daily_anchors` · `daily_shown` · `annotations` · `pdf_annotations` · `reading_proposals` ·
`reading_targets` · `discovery_profiles` · `gate_log`.

**`object_links.target_kind`** is `doc` (keyed by `source_uri`), `entity` (keyed by
`entity_key(name,type)`), or `object` (keyed by `str(object_id)`). Note that `object` covers
*both* project links and thread↔thread links — filter on the target's `type` if you mean one of
them specifically.

---

## 6. Ingest

`ingest_pipeline.py`. Idempotent by hash; per-document transaction; a failure quarantines one
document and the batch continues.

```
source (vault/incoming/<category>/… or a tracked repo)
  → hash; existing → skip
  → raw-store copy
  → EXTRACT (extract/)
      pdf   font/shape section heuristics · ToC excised · de-hyphenation · damage+math
            detection → GOT math-OCR with deterministic QC fallback · figure detection
      pptx  one span per slide, real slide numbers, notes, soffice render for figures
      code  repo = doc, files = sections, def-granular chunks with line provenance, call
            graph, AST entities + an LLM domain-concept pass over the repo NARRATIVE
            (README/synthesis/summaries — never raw code). `.md` is first-class here:
            for a project repo the READMEs are often the most informative content.
  → LLM PASSES (local qwen, pydantic-validated, bounded repair with temperature escalation)
      section summaries (grounding guard) · propositions (anti-meta + rejection filters) ·
      entities (normalisation, noise/grounding filters) · doc synthesis · gap flagging
  → FIGURES (batched after text — one VRAM swap per doc)
  → EMBED (nomic, 768-dim, LOCKED — changing it invalidates every vector)
  → WRITE all levels in one transaction
```

**Hard rule:** every structured LLM output is pydantic-validated with bounded repair. The pipeline
never silently writes garbage and never aborts a batch on one bad document.

Extracted PDF text is stripped of control characters (`extract/pdf._strip_controls`): maths
PDFs encode symbols in fonts whose code points collide with C0 controls, so |x - a| < e
extracts as `jx \x00 aj < \x11`. Invisible on every surface, and a NUL cannot go in argv —
see `agent/claude.py`. An EMPTY file is `unsupported`, not quarantined: there is nothing to
extract, so it is not a failed extraction.

A pre-ingest content gate quarantines single-column data dumps (an English dictionary was once
ingested and the synthesis pass hallucinated 4,129 phantom entities).

---

## 7. Retrieval

`retrieve/` + `query.py`. One generation call at the end.

```
embed query
  ├─ propositions top-10 · chunks top-20 · sections top-5 · figures top-8   (dense)
  ├─ lexical FTS5/BM25 over chunks        (exact symbols/tickers dense blurs)
  ├─ path-anchored: code sections whose file stem is named in the query
  └─ entity-anchored: alias-aware — any variant surfaces sections naming any sibling
merge → cross-encoder rerank → select() with diversity rules (per-(section,kind) cap,
        child-redundancy demotion, per-doc cap; all soft with refill)
  → confidence: calibrated floor — FLAG, NEVER FILTER; facet-aware so a bridge query whose
    facets are each covered does not get a "weak coverage" banner
  → hierarchical expansion (parent summary + doc synthesis; plain joins, no inference)
  → assemble coarse-to-fine under a token budget; citations deduped
  → one Claude call; top-3 figures attach as real images
```

**Multi-query expansion** rephrases a bridge-shaped or low-confidence query into other
disciplinary vocabularies and reranks each candidate against the variant that found it.
**Facet decomposition** splits conjunctive queries so an under-represented facet is not crowded
out. Both make the labelled eval non-deterministic at the margin — promote a label only on
repeated observation.

`retrieve/threads.py` joins the owner's promoted threads on during *expansion*, not as a
retrieval arm — a parallel arm would put the same text in the pool twice and double-count against
the per-doc cap.

---

## 8. Link layer

`locus/link/`, runs after ingest, outside the ingest/retrieval spine. Manual/nightly, billed.

- **`aliases.py`** — deterministic tiers first (casefold, punctuation, attested acronym
  expansion, attested cross-doc plural — same-type only), then embedding-blocked lookalike
  clusters adjudicated by a model. Guards override the model: min-merge-length 4, same-section
  co-occurrence never merges, canonical snapped to a real member surface, code docs excluded,
  oversize clusters **chunked, not dropped** (the cap is a cost guard, and applying it as a
  judgement silently lost real concepts like *Fama-French factors*). One unreadable reply skips
  its cluster; it never aborts a rebuild. Verdicts cached ⇒ re-runs ≈ 0 calls.
- **`related.py`** — docs ranked by shared canonicals, joins-only, IDF-weighted (each shared name
  contributes 1/doc_freq). Code symbols score only for code↔code pairs and only at a 0.1
  tiebreak; a semantic arm (mean-pooled section vectors) is **tail-additive** — it can fill a
  slot after every entity neighbour, never displace one. `non_topical_names()` is the shared
  definition of "too generic to be a concept" (boilerplate, code symbols, document titles);
  reuse it rather than writing a second one.
- **`threads.py`** — two threads link when they name the same canonical concept, checkable by
  reading both. Guards: canonical spans ≥2 docs, ≥5 chars, `non_topical_names` applies, only his
  text is matched.
- **`projects.py`** — which project a piece of his writing is about. Deterministic tier (every
  distinctive title token present) then a cosine floor.
- **`connect.py`** — the written reason two documents belong together, composed overnight and
  stored. Three framings, one per arm: *an idea for this repo* (project), *what he read*
  (capture), *what he already studied* (the coursework bridge). The model must name which shared
  concept it built on and that pick is verified against the offered list; a pick that was not
  offered is dropped. `[agent].connect_model` defaults to **sonnet while `[agent].model` stays
  haiku** — model choice here is a safety property, not a fluency one: on a junk pair Haiku
  bluffed confidently twice and Sonnet answered `NO_CONNECTION`. Candidate pairs come from
  `compose_daily.connection_candidates`, so the page and the overnight writer can never disagree
  about what a connection is.

---

## 9. Agent layer

Turns the engine into a learning and project-development tool. State lives in its own tables.

**Capture.** `capture/rmdoc.py` reads stroke geometry from the device's cloud copy. Shape decides
the gesture; position only sets `in_margin`. Hand underlines sit *below* their glyphs, so the
underline band searches *above* the stroke; highlights sit *on* their glyphs and are clustered
one-per-line (`_split_highlights`). `capture/annotate.py` stores both `covered_text` (what the ink
covered) and `line_text` (the full line) — **marginalia is deictic**, and for a note like "what
does this mean here?" the line *is* the content of the question. `capture/loop_b.py` runs the
whole chain on the half-hourly capture timer for the Reading folders Loop A excludes: changed
document → marks stored → new handwriting transcribed (bounded per run). Marks map to corpus
documents **by content hash of the bundle's PDF bytes** — computed, never remembered; an
unmatched (un-ingested) document keys by device path with a loud log and an
`unmapped_to_corpus` stat. A document he *chose* (moved into `Reading/In-Progress` or `Finished`)
with no corpus match is **ingested from the bundle's own bytes** in the same tick and its earlier
device-path marks re-keyed onto the new `source_uri`; `Proposed` never auto-ingests, because most
proposals are rejected. Once a document is corpus-mapped, his margin notes are promoted to a
reading-notes file automatically (`agent/promote.promote_reading_notes`), which `notes-sync`
ingests on the next tick — so marginalia reaches retrieval with no command typed.
`reading/sweep.py` is the older, geometry-only hourly cloud pull over `reading_targets`; the
overlap with Loop B is deliberate redundancy, not duplication — both write through the same
idempotent upsert.

**Intent.** `capture/intent.py` classifies a mark as `important` / `not_understood` / `idea`.
Below `[capture].intent_confidence_floor` nothing happens except that it becomes a `locus decide`
item — acting on a low guess silently is the correction step removed. An intent the owner set is
never re-guessed. **A mark with no covered text and no note is not pending**: `classify` rejects
it before making a call, so it can never leave the pool, and 28 such blank marks (highlights over
figures, §14) sat at the head of an id-ordered, 100-per-run queue on 2026-08-06 and spent the cap
before it reached his three newest written questions. The filter is on current content, not a skip
flag, so a mark that gains a note later re-enters by itself. `locus intent --act` also runs
**before** `review --answer-marks` in `locus-maintain` — it writes what that step reads, and
running after it put a two-day lag between writing a question and seeing it answered.

**Structure.** `structure/propose.py` proposes objects and belief positions; the owner blesses in
`locus decide`. The precision bar is the module: concepts must resolve through `entity_aliases` to
a canonical spanning ≥2 documents and survive `non_topical_names`; every object carries ≥1
grounding link; ≤3 per document. **Owner-authored is about provenance, not category** —
`state.owner_authored_sql` is the one definition (path under `vault/notes/` first, category as
fallback), and a promoted thread is excluded so the loop cannot feed itself.

**Learn.** A margin question the corpus cannot ground is **parked** after
`_MAX_ANSWER_ATTEMPTS` failures (migration 0035, `pdf_annotations.answer_attempts`) and
named by `locus status`; without it the mark is re-offered every night forever, because
`pending_questions` selects on the ABSENCE of a stored answer and grounded-or-silent stores
nothing on failure — two correct rules composing into a permanent occupant of a bounded
queue. Parking is a pause, not a verdict: `locus review --retry-parked` clears the counters,
and it is worth running after an ingest batch, since the corpus is not what it was.
`learn/answers.py` answers margin questions from the corpus (evidence excludes the
reading-notes aggregation, which *contains* the question). `learn/review.py` runs SM-2 and writes
concept cards — question and answer are generated **together from one evidence set**, because when
they were produced by different mechanisms they silently stopped matching.

**Evolve.** `evolve/trajectory.py` builds the dated position chain (a pure join) and judges
tensions. A tension must classify as `factual`/`methodological`/`predictive` and state what he
would do differently; `terminological` and `emphasis` are dropped. `_JUDGE_VERSION` is the cache
key — **bump it whenever the prompt changes**, or cached verdicts outlive the judge that made them.

**Surface.** `surface/grounding.py` assembles a citable evidence set (deterministic, free);
`critique.py` and `synthesise.py` hand it to a model and drop any claim citing a key they were not
given.

---

## 10. The daily page

`agent/compose_daily.py` composes; `reading/md2pdf.py` renders; `agent/pull_daily.py` reads ink
back. Pages are named for what he *does* on them, one section per physical page. An empty section
is omitted entirely, and omitting it takes its page break with it:

| Page | Anchors | Source | Writing region |
| --- | --- | --- | --- |
| Develop | `D1..` | active question/idea objects (his own threads) | ruled + tick |
| Consider | `C1..` | `connection_notes` | ruled + tick |
| Answered | `A1..` | `mark_answers` — margin questions answered from his corpus | none |
| Recall | `L1..` | `review_schedule` (question on its page, answer on the back) | ruled + tick |
| back page | `Q1` | always present: open region, status line, recall answers | ruled |

**There is no Read page.** Two days of use showed he never used it to make a reading decision — he
used it to have ideas *about* papers, which is Develop's job. The per-proposal rationale moved to
the shelf itself: `locus reading-why` renders a `Proposals` document into `Reading/Proposed`,
fingerprinted over the proposals so an unchanged shelf costs nothing. `build_readings` /
`build_rereads` / `_explains` still exist and are used by `learn/reread.py`; nothing on the page
calls them. The re-read slot was **retired, not recalibrated** — the gate log recorded 190/190
rejections with a best-ever score of 1.917 against a floor of 2.5, and 1.917 *is* the known-wrong
match, so lowering the floor buys only the pun.

**There is no Check-this section** (2026-08-06). Tensions are still judged and stored overnight
and still readable via `locus evolution`, but after two days of live cards he judged the section
not worth its seat, and the seat went to a third connection. `build_challenge` is kept and
unwired; the Consider page prints one section and so carries no subsection label.

**The status line is present tense.** `health.check` reports what is broken NOW: a failure a later
run of the same kind has since recovered from moves to `Health.recovered`, which `locus status`
prints and the page does not. Otherwise one transient break shouts in capitals all day — `review`
died once at 03:37, ran clean eight times after, and was still on the page that afternoon along
with the `maintain` unit it took down. Unit failures match runs by name (`locus-maintain.service`
→ kind `maintain`), which is the contract `locus record <kind> --ok` relies on.

**Recall pages the due queue, never a fixed window.** A card shown but never graded keeps its
`due`, so it keeps both its retired `item_key` (invariant 9) and its place at the head of
`due_items`. Seventeen of them accumulated and filled every window the page asked for, so Recall
went 4 → 4 → 1 → 0 items over four days with forty eligible cards behind them — silently, because
an empty section is simply omitted. `build_recalls` now scans until it has `limit` unseen items
and records what the ledger rejected under the `daily.recall_unseen` gate.

**Anchors are load-bearing.** `pull_daily` routes ink by the *printed label* — vision is asked for
"the label exactly as printed" — and `annotations` is `UNIQUE(page_date, anchor)`. Removing or
duplicating a label breaks the two-way loop. Letters name the page they sit on, but routing
dispatches on `daily_anchors.kind`, never on the letter — which is why Develop's letter could be
corrected from `I` (left over from the section's old name, Ideas) to `D` without migrating
anything: each delivered page's own `daily_anchors` rows are what routes it.

**DEVELOP ranks on project links, then staleness.** Ranking on `updated_at` alone put a
22-character to-do at the head of the section.

**The page is fitted by rendering it, not by estimating.** `agent/layout.py` runs between
`compose` and `render`: for each section it renders that section alone, counts pages, and grows
it while it still measures one page — **more cards first, then more ruled lines** on the cards it
kept, never below the floor each section prints today (develop 3 · consider 3 · recall 2 ·
answered 3 · open 4). A render costs ~40ms, so a whole fit is under a second; the one thing that
must not be paid twice is `connection_candidates` (~39s), which is why `compose` leaves its
candidates on `page.pool` for the fit to trim. The fit re-runs `assign_anchors`, because what it
drops or adds must be numbered before `persist` and `record_shown` — a label printed against one
card and stored against another routes his ink to the wrong record. It degrades to the static
estimate if anything fails, and `--no-render` skips it. Live effect (2026-08-06): pages went from
~62% to 87–94% full, develop 3 → 4 cards.

**The static budgets are now the fallback, not the layout.** `_FIT`, `_lines_for`/`_LINE_BUDGET`,
`_pack` and `_CONNECT_CHAR_BUDGET` were each set by rendering a real PDF once and then frozen,
which cannot be right for both a day of short cards and a day of long ones; they still size the
page for every caller that cannot render (`decide.queue.page_keys`, most tests). A test asserts a
full page does not overflow into a fifth.
Line *spacing* is measured the same way and only from the rendered geometry:
`scripts/analysis/daily_rule_spacing.py` prints the gap between consecutive rules, which is how
the keep-this line was caught sitting 30.2pt below its neighbour where every other rule sat
19.8pt apart. Anything drawn beside a rule belongs in `md2pdf.PageGeometry._keep_rule`, where it
shares the one `rule_gap_em` — spelled out in the composer's markdown it silently drifts, since
Typst collapses `above` against the previous block and a tick box makes the block ~10pt tall.

`/Daily` is an inbox: a page stays loose until it has ink, then archives to `/Daily/YYYY-MM`.

---

## 11. Evaluation

Four suites (`locus eval --suite judge|math|retrieval|full`) plus a deterministic audit.

- **audit** (`locus audit`, no API) — re-applies ingest hygiene predicates to stored rows; corpus
  distribution; alias substrate block.
- **judge** — Claude scores stored extractions against source (6 dimensions).
- **math** — flagged pages rendered to PNG and judged against stored text. Doubles as the
  VRAM-regression canary.
- **retrieval** — labelled recall@8, MRR, cross-domain banner rate, file-level recall for code,
  links_recall over labelled pairs. Labels live in `locus/eval/retrieval_eval.py`; **extend them
  whenever the corpus grows.**

Baselines to hold: recall@8 1.000 · cross-domain 1.000 · banner 0.000 · file_recall 1.000 ·
links_recall 1.000 · mrr ~0.80 · judge 4.0–4.4 · math 0.87–0.95. Weakest dimension is judge entity
recall (~3.4), an extraction ceiling.

**A frozen number rising means nothing; recall holding on a set that GREW is the honest gate.**

---

## 12. Repository layout

```
locus/
├── CLAUDE.md · README.md · docs/
├── pyproject.toml (uv; extras: rerank, mathocr, reading, tui) · alembic.ini · config.toml
├── locus/
│   ├── cli.py              # the product surface (all commands)
│   ├── config.py           # typed config; API key via env/.env only
│   ├── db/                 # connection, migrate, migrations/ (0001–0034)
│   ├── extract/            # pdf · mathocr · figures_detect · docx · pptx · textdoc · code
│   ├── ingest/             # llm · summarize · propositions · entities · concepts · synthesis
│   │                       #   gaps · chunk · embed · figures · llamacpp
│   ├── ingest_pipeline.py · watcher.py · sync.py · repo_sync.py · notes_sync.py
│   ├── retrieve/           # search · rerank · expand · assemble · pipeline · multiquery
│   │                       #   threads · figure_images
│   ├── link/               # aliases · adjudicate · related · threads · projects · connect
│   ├── agent/              # claude (the one `claude -p` runner) · journal · budget · state
│   │                       #   layout (fits each section by rendering it)
│   │                       #   compose_daily · pull_daily · promote
│   ├── capture/            # remarkable · rmdoc · annotate · mark_text · intent · transcribe
│   │                       #   loop_a (notes) · loop_b (reading) · fillin · conversations
│   │                       #   device_migrate
│   ├── learn/              # answers · review · practice · gaps · reread
│   ├── reading/            # proposals · rationale · relevance · sweep · accept · deliver
│   │                       #   deliver_remarkable · md2pdf · watch
│   ├── discover/           # arxiv · openalex · citations · profiles · queries · rank · judge · why
│   ├── evolve/ structure/ surface/ decide/ enrich/ vault/ export/ eval/
│   ├── observe/gates.py    # what each threshold rejected (§3)
│   ├── health.py · status.py · backup.py · query.py · retitle.py · mcp_server.py
│   ├── ingest_lock.py      # the advisory flock: one ingest process at a time
├── deploy/systemd/         # timers: maintain, daily, daily-pull, capture, discover-*, backup
├── scripts/analysis/       # one-off measurement scripts, kept as evidence
├── tests/                  # ~1,057 model-free-by-default tests
└── vault/                  # incoming/ · raw/ · notes/ · backups/ · locus.db
```

---

## 13. Conventions

- Python 3.11+, uv-managed. Type hints mandatory. Small explicit functions.
- **Docstrings carry the decision log.** State the non-obvious assumption and the *why* — this is
  how a future change avoids re-introducing a fixed bug. Record designs that were **built,
  measured and rejected**, with the measurement.
- Structured LLM I/O through pydantic, never raw dicts. Every external call has explicit error
  handling and degrades rather than aborting.
- Tests are model-free by default (injected fake clients, seeded tmp DBs). Keep the suite green.
- No secrets in code or committed config. `ANTHROPIC_API_KEY` via env/.env.
- All tunables in `config.toml`; optional sections default cleanly. `config.toml` is gitignored —
  document new settings in `config.example.toml`.
- **`agent/claude.py` is the one `claude -p` runner** and is env-scrubbed so it uses subscription
  OAuth rather than the metered key. This has been mis-wired twice. It also **scrubs control
  characters from every prompt**: the prompt is an argv element, so a NUL raises `ValueError`
  before the process spawns — not a `ClaudeError`, so no `except ClaudeError` degrade path catches
  it. Maths PDFs extract symbol-font glyphs as control codes, and three calculus documents failed
  the structure pass on *every* run because of it.
- **Tests must not read the live `config.toml`.** It is gitignored, so a test that inherits it
  passes or fails per machine: two connection tests asserting paper-vs-coursework ranking broke
  the day `[agent].connect_idea_projects` gained entries, and nowhere else. Pin what you depend on
  (`_no_idea_allowlist` in `tests/test_compose_daily.py` is the pattern).

### Operational rules

- One ingest process at a time (flock).
- Run the math suite after any VRAM-choreography change.
- Run `locus link` after ingest batches.
- **Restart `locus mcp` after any retrieval change** — a long-lived server runs code from its
  start time. Compare its build stamp with `git rev-parse --short HEAD`.
- Quarantines are bugs to triage, not casualties — which is only true because **unsupported
  is filed separately** (`incoming/.unsupported/`, `watcher.UNSUPPORTED_DIRNAME`). A PNG, a
  `uv.lock` and an empty README are not documents and never will be; filing them with real
  failures kept 8 permanent files in the triage pile and made `locus status` warn every day
  about something that could never be actioned.
- Eval labels grow with the corpus.
- **Deploying a systemd unit means copying it to `~/.config/systemd/user/` and reloading.**
  Editing the repo copy alone changes nothing.

### Out of scope

Custom GUI · cloud storage of corpus content · multi-user/auth · local models > ~8B.

---

## 14. Standing decisions and known limits

- **§11.B — the weakest model owns high-value passes.** 8B-quantised quality is the ceiling on
  summaries, propositions, entities and VLM descriptions. Mitigated by validation, grounding
  guards and raw-chunk co-assembly. Revisit per-pass routing only on eval evidence.
- **The corpus is ~62% coursework (144/232), deliberately.** It was measured and the mitigations
  hold: the retrieval penalty was inert, the proposer's gate lets through 3 of 82 coursework-only
  concepts, and no thread link is coursework-polluted. The maths bridging into quant work lives
  there and nowhere else — and since the CONNECT bridge arm, coursework finally *reaches* a
  surface: a coursework connection is now deliberate output (the "you already studied this"
  framing), not pollution.
- **Concept fragmentation is not a defect.** ~17% of canonicals span ≥2 documents; promotion tiers
  were measured and rejected (sub-phrase promotion is wrong in both directions, cross-type merging
  yields +16). A heterogeneous corpus contains mostly document-specific vocabulary.
- **Brute-force KNN** is fine at this scale; add an ANN index when the count warning fires.
- **Content-hash idempotency is whitespace-sensitive.**
- **Model self-assessment is not a usable signal.** Asking a model which key it cited, or to grade
  its own output, produces something that looks like a filter and is not. Verify independently or
  deterministically.
- **Familiarity is a tiebreaker, not a term.** `discover/rank` subtracts familiarity at weight
  0.25, not 1.0: at 1.0 it destroyed the ranking, because subtracting familiarity presumes a
  corpus dense enough for "he already has this" to be often true. Raise it as density grows.
- **Relevance gates; novelty sorts.** Both discovery scalings of `fit - familiarity` as co-equal
  summands failed — raw cosines are packed into a 0.65–0.75 band, and standardised, novelty
  dominates and fills the list with papers unlike anything at all.
- **Open:** highlights over figures capture nothing (6 of 24 on a live document); page-edge stamps
  can leak into a capture; the discovery flywheel needs ~4 resolved judgements per channel before
  its prior activates (live: every subject sits at 0–1, so `subject_prior` is inert); an
  annotated document still sitting in `Reading/Proposed` keys its marks by device path until he
  moves it; a same-day daily rebuild delivered with `replace=True` keeps the device's old
  per-page records (`deliver_remarkable.deliver_pdf`), so a page-count change leaves the new
  last page with no page entry — fixable only by deleting the device copy, which is safe only
  when it carries no ink.
