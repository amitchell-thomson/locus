# Code concept extraction — implementation plan

> Status: **Phase 1 SHIPPED** (2026-06-09); Phases 2–3 planning. Closes the corpus's biggest
> *linking* gap: code repos carried function names, not the domain concepts they implement, so
> projects couldn't link to the papers/coursework they're built on. This is **THE primary use
> case** (CLAUDE.md §1.2 — Link), so the acceptance bar is a measured cross-domain link.
>
> **Phase 1 results (live corpus):** the concept pass (`ingest/concepts.py`) + backfill +
> `locus link` (0 API spend — the concepts were canonicals the papers already established) took
> the projects from **6/12 linking to nothing → 0 orphans** in the Obsidian graph. The owner's
> quant projects now link to each other on shared concepts (regime-ml ↔ tanker-flow ↔
> downside-risk: regime switching / Markov-switching / mean reversion), verified mutual in the
> related top-5 and added to the eval (`links_recall` 1.000 over 13 pairs). A README/CLAUDE.md
> overflow bug (88-section repos blew num_ctx and degraded to one junk concept) was fixed by
> bounding the narrative; a base-concept prompt lever (mirroring the entity pass) recovered
> regime-ml. **Honest boundary:** project→PAPER links now *surface* (regime-ml reaches the
> SVAR/Markov papers via the base concept "Markov model") but at shared=1, **rank 6–8** —
> below the top-5, crowded out by the stronger cross-project links. Lifting a paper into the
> project's top-5 needs fuzzy clustering of the code-only surfaces ("regime switching" vs the
> papers' "regime shift") — that is **Phase 2** (§5). The AST `concept→tool` re-typing (§4) was
> deferred (eval-sensitive, not required for the links above).

---

## 1. The gap (measured, 2026-06-09)

Code repos are the *densest* source type by raw volume but the *thinnest* by linkable
concept. Measured over the live 12-repo / 291-doc corpus:

| per doc | code | pdf | what it means |
|---|---|---|---|
| chunks | 316 | 23 | code is richly chunked (good for symbol/implementation retrieval) |
| entities | 143 | 70 | …but they are **AST identifiers**, not concepts |
| **propositions** | **0** | 58.8 | skipped by design (§11.B) — fine |

The entities a repo contributes are `fetch_feed`, `build_article_context`, `ZScore`,
`MovingAverage` — function names (`type='method'`) and class names (`type='concept'`). They
are *identifiers*, and `link/related.py` deliberately filters identifier-shaped names as
boilerplate. The consequences, measured:

- **6 of 12 code docs (50%) link to nothing** — vs **259/259 PDFs, 10/10 docx, 5/5 notebooks**.
- **All 3 orphans in the Obsidian projection are code/project docs** — including
  *"Alpha Fund — covariance, portfolio stats, **Black-Scholes** Monte Carlo"*, which shares
  **zero** canonical entities with the finance papers that cover exactly those concepts,
  because its entities are function names.
- **63 README/CLAUDE.md/design-doc sections inside code repos yield 0 entities** — pure domain
  narrative (`llm_entities=False` for code; AST parses only `.py`), contributing nothing.

So a repo *says* what it is — doc 172's synthesis reads *"daily global news summaries with
market updates… using Claude AI"* — but that understanding never becomes a **linkable entity**
in the same canonical space as the papers. The repo captures the *how* (function names); the
*what* (regime switching, Black-Scholes, LNG routing, AIS) is unextracted.

This is **extraction loss** (CLAUDE.md principle 8 — not recoverable, outranks cosmetic work)
on the **Link** use case (§1.2), which the owner calls the primary one.

## 2. Key design insight

The reason code skips LLM passes is sound: *"claims-from-raw-code is the weakest 8B task"*
(`ingest_pipeline.py:190`, §11.B). **This plan does not reverse that.** It adds a pass that
reads the repo's **narrative** — README/markdown, package & class docstrings, the existing
file summaries, and the doc synthesis — none of which is raw code, all of which is exactly the
doc/section-level prose where an 8B model is reliable (the same surface `summarize`/`synthesis`
already extract from successfully for code, 357/357 and 12/12).

> **Concept extraction from a repo's narrative is the same task class as paper entity
> extraction, not the same as claims-from-raw-code.** That is what makes it safe under §11.B
> and consistent with the standing decision to skip propositions.

It also stays within principle 5 (local models for ingest): extraction runs on local qwen;
the *merge* decision that could corrupt the durable link substrate still goes through
`locus link`'s Claude adjudication, exactly as today.

## 3. The concept-entity pass

A new ingest pass, **`ingest/concepts.py`**, run only for `source_type='code'` (the profile
axis that all of regime/tanker/alpha/quant-data share).

**Inputs** (all already extracted — narrative, never raw code):
- the doc **synthesis** (`thesis/method/result/limitations`);
- the **README / `.md` section summaries + text** (the 63 currently-barren narrative sections);
- per-`.py`-file **section summaries** and **module/class docstrings** (already in chunks).

**Output** — a validated pydantic `ConceptExtraction`:
```python
class Concept(BaseModel):
    name: str          # the domain concept as written ("regime switching", "Black-Scholes")
    type: Literal["concept", "method", "tool", "dataset", "metric", "theorem"]
    evidence: str      # the span it was grounded in (for the grounding guard)

class ConceptExtraction(BaseModel):
    concepts: list[Concept]
```
The prompt asks for *the techniques, models, datasets, and domain ideas this project
implements or studies* — not its code structure. `author/ticker/organization/other` are out of
scope here (they don't bridge to research). Concepts anchor to the section they came from
(README/module section), or `section_id=NULL` for synthesis-level concepts (the column is
already nullable).

**Guards** (reuse the existing entity-pass machinery in `ingest/entities.py`):
- **grounding** — a concept's distinctive stem must overlap its `evidence`/input narrative
  (the `summarize` grounding-guard predicate); ungrounded concepts are dropped, never written.
- **generic stoplist** — drop corpus-useless concepts (`python`, `api`, `data`, `function`,
  `library`, `framework`, `algorithm`, `model`…) that would link everything to everything.
  The corpus-aware stop-entity guard (`stop_doc_freq_ratio`) is the second line of defence.
- **normalization + dedup** — casing/plural merge via the existing entity normaliser; cap per
  doc (config) to bound a runaway README.
- pydantic validation + bounded repair, like every other structured pass (§7 hard rule).

Cached in `pass_cache` keyed on `(inputs_hash, model, PROMPT_VERSION)` — re-ingests and
backfills only pay for changed repos (§7 idempotency).

## 4. Keeping concepts and identifiers separate

Today AST types **classes as `concept`** (`ZScore`, `BaseClient`) — so the `concept` type is
already polluted with identifiers. To make `concept` mean *domain concept* corpus-wide (so the
clustering and Obsidian entity-type folders are honest):

- **Re-type AST class entities `concept → tool`** (`extract/code.py`, one line: `ClassDef →
  type="tool"`). A class is a code construct, nearer `tool` than a domain concept; functions
  stay `method`. After this, every `type='concept'` entity on a code doc came from the new
  pass.

This is eval-sensitive (it shifts entity types) — gated behind the retrieval eval and applied
together with the backfill. It is reversible and not strictly required for linking to work
(see §5), so it can land in Phase 1 or defer to Phase 2 if the eval flags churn.

## 5. How linking actually happens (and what does / doesn't change)

The win comes almost entirely from the **deterministic** alias tiers, which need **no change**:

- `locus link`'s identity/casefold/punct/acronym tiers merge surfaces **regardless of**
  `code_only`. The instant the regime repo carries a `concept` "Markov switching" that matches
  a paper's "Markov switching" (exact or casefold), they snap to one canonical →
  `related_documents` shares it → **repo links to paper.** No `aliases.py` change required for
  the exact/casefold case, which covers the bulk of real matches.
- `related.py`'s `_BARE_IDENT_FILTER` stays — it still correctly removes AST identifiers, and
  real concepts pass it untouched (verified against the targets: `regime switching`,
  `Black-Scholes`, `Monte Carlo`, `LNG`, `AIS`, `covariance` all clear the filter).

**Residual gap → Phase 2:** the `code_only` flag (`aliases.py:63,167,454`) excludes
surfaces seen *only* on code docs from the **fuzzy LLM** tier. So a code concept that only
*fuzzily* matches a paper ("Black-Scholes model" vs "Black-Scholes") won't merge. Fix:
make the eligibility `not n.code_only or n.type == 'concept'` so code-only **concepts** are
fuzzy-clusterable while code identifiers stay excluded. Deferred because exact/casefold
already delivers the primary use case; fuzzy is the long tail.

## 6. Schema

**No migration.** `entities` already supports `type='concept'` (free-text type, nullable
`section_id`, `UNIQUE(doc_id, section_id, name, type)`). The pass writes more entity rows;
nothing structural changes. Entities are not embedded (no `entity_vectors`), so there is no
re-embed cost — which makes the backfill cheap (§8).

## 7. Config

A small optional `[concepts]` section (defaults clean, per §14):
```toml
[concepts]
enabled            = true   # run the code concept-entity pass at ingest
max_per_doc        = 40     # cap on concepts written per repo (runaway-README guard)
# generic terms never written as concepts (corpus-useless link hubs)
stoplist           = ["python","api","data","function","class","library","framework","algorithm","model"]
```
Wired into the code `_PassProfile` as a new `concept_entities` flag (alongside
`propositions=False, llm_entities=False`), so `pass_profile()` stays the single source of
truth and `audit` keeps reporting reality (§11 "0 props is BY DESIGN").

## 8. Backfill (the existing 12 repos)

`scripts/backfill_code_concepts.py` — for each `source_type='code'` doc, run the pass over its
**already-stored** synthesis + summaries + README text and insert `concept` entities. **No
re-extract, no re-embed, no re-ingest** (the narrative inputs are already in the DB; entities
aren't vectors). Idempotent via `pass_cache` + the entity UNIQUE constraint. Then:

1. `scripts/backfill_code_concepts.py` (writes concept entities)
2. `locus link` (rebuild `entity_aliases` — billed Claude adjudication, but cached/incremental)
3. `locus export-obsidian` (the orphans should now link)

Going forward the pass runs at ingest, so new repos arrive linked.

## 9. Retrieval bonus (not just the graph)

The entity-anchored retrieval arm (§8, alias-aware) benefits for free: a query *"regime
switching"* currently entity-anchors to papers only; once the regime repo carries that
concept, the **same query surfaces the implementation**. So this improves the Query use case
(#1) and the Link use case (#2) from one pass — directly serving the owner's "link my projects
to the papers I've read" workflow from both the retrieval and projection sides.

## 10. Eval — the acceptance metric

This feature is **defined by** measured cross-domain links, so the eval leads, not trails.
Add to `eval/retrieval_eval.py`:

- **`links_recall` pairs** (the bar): `regime-conditioned-equity-ml ↔` the regime/Markov-switching
  papers (*"Enhancing Regime Shift Detection"*, *"Inspectable Neural Markov Models"*, the SVAR
  Markov-switching paper); `tanker-flow ↔` the *"Multistart LNS for LNG transportation"* paper
  (LNG / maritime routing / AIS); `alpha-fund ↔` the Black-Scholes/Monte-Carlo/portfolio
  papers. **Pre-feature these score 0** (the docs are orphans today — that is the baseline to
  beat); post-feature they must link via a shared canonical concept.
- **a retrieval label**: *"regime switching implementation"* should surface the regime repo
  (entity-anchored), proving §9.

Every label verified live before commit (the `retrieval_eval` convention). Hold the existing
baselines (recall@k 1.000, links_recall 1.000) — the new pairs extend the set, they don't
relax it.

## 11. Tests (model-free)

- **concept pass** with an injected fake LLM: grounded concepts written; an **ungrounded**
  concept rejected by the grounding guard; a **stoplist** term dropped; normalization/dedup.
- **pass profile**: `pass_profile('code')` runs `concept_entities` while still skipping
  `propositions`/`llm_entities` (regression guard on the §11.B decision).
- **integration** (seeded, no model): a code doc and a paper doc each carrying surface
  "Black-Scholes" → after a deterministic `build_aliases`, `related_documents` links them.
  This is the unit-level proof of the primary use case.
- **AST re-typing** (if Phase 1): a `ClassDef` yields `type='tool'`, a `FunctionDef`
  `type='method'`.

## 12. Phasing

1. **Phase 1 — the pass + backfill + eval. ✅ SHIPPED 2026-06-09.** Concept pass for code,
   backfill the 12 repos, `locus link`, add the verified cross-project eval pairs. Deterministic
   alias tiers do the linking; 0 orphans, `links_recall` 1.000. AST `concept→tool` re-typing
   deferred. Outcome + the project→paper boundary documented in the status header.
2. **Phase 2 — fuzzy code concepts.** Relax `code_only` for `type='concept'` so near-miss
   surfaces ("Black-Scholes model" ↔ "Black-Scholes") cluster via the LLM tier.
3. **Phase 3 — depth.** Richer typing (method/dataset/metric split), concept extraction from
   notebooks if they show the same gap, and co-occurrence edges in the Obsidian projection.

## 13. Open questions / risks

- **Hallucinated concepts.** An 8B model could over-claim from an aspirational README. Mitigated
  by the grounding guard (concept must be attested in the narrative) + the `evidence` field +
  the stop-entity guard. Watch the audit's noise-entity counter after the backfill.
- **AST re-typing eval churn.** Re-typing classes `concept→tool` shifts entity-type
  distributions; gate on the retrieval eval and keep it reversible. Defer to Phase 2 if it
  destabilises.
- **README quality variance.** Some repos have thin READMEs; the synthesis + file summaries are
  the floor (always present), so a thin README degrades gracefully rather than failing.
- **Concept granularity.** "time series" (too broad) vs "regime-switching volatility model"
  (good). The stoplist + `max_per_doc` + the corpus-aware stop-entity guard tune this; start
  conservative (a missed concept is fragmentation; a junk concept is a false link).
