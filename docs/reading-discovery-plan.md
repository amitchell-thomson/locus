# Reading discovery — Phase 4

**Status:** SHIPPED, 2026-08-01 — all three steps, live on systemd timers.

> This document is the DESIGN as it was reasoned out. Three rounds of measurement then overtook
> parts of it, and the corrections are recorded in `## What measurement changed` at the end —
> read that alongside the design, because several confident calls here turned out to be wrong.
**Supersedes:** the 2026-07-30 design note of the same name (kept in git history).
**Date:** 2026-07-31
**Referenced from:** `locus/learn/reread.py`, `locus/agent/compose_daily.py` (`build_readings`).

---

## 1. Why this exists, restated as a density problem

The earlier note framed this as *discovery vs. gap-filling*. That framing is right, but it is not
the binding constraint. The binding constraint is **density**:

| category | docs |
|---|---|
| coursework | 144 |
| career | 16 |
| **project** | **15** |
| **note** | **15** |
| **paper** | **13** |
| **total** | **203** |

Measured 2026-07-31. Retrieval can only choose from what is there, and 71% of what is there is
Oxford engineering coursework. Ranking work has gone as far as it can — `learn/reread.
concept_weight` now weights gap concepts by rarity, which stopped coursework owning the read-next
slot, and the `rough_penalty` sweep (2026-07-29) established that the remaining note-retrieval
misses are an extraction ceiling, not a ranking one. **Neither of those adds a single quant
document.** This is the fix, and it is the first thing in Locus whose job is to make the corpus
bigger rather than to rank it better.

## 2. The two-cadence shape

The earlier note had one pipeline. There are really two, and they differ in almost every respect:

| | **Papers** | **Books** |
|---|---|---|
| cadence | weekly | monthly-ish, or slower |
| open access | usually | rarely |
| proposal artifact | **the real PDF** | **a stub page** |
| ingest | on accept, immediately | when he supplies the file |
| volume | ≤3 held in `Proposed` | **exactly 1** at a time |
| dominant signal | citations, then arXiv currency | marginalia; references inside books |

The book path is not a slower copy of the paper path. A book is a month of his time, so the right
output is *one considered suggestion*, argued for, that he can ignore at no cost. A feed of book
recommendations is precisely the failure this system exists to avoid.

The paper path can be a genuine pipeline, because the marginal cost of a proposed paper is one
swipe.

## 3. What I measured, and how it revises the design

Everything here is from the live DB and the raw PDFs, not from assumption.

**(a) The reference-extraction pool is 15 documents, not 20+.** The earlier note proposed
"reference extraction over the top ~20 highest-engagement corpus PDFs". There are 13 `paper` PDFs
and 2 `project` PDFs (both Optibook manuals, no bibliography). The other 141 PDFs are coursework
handouts. The honest pool is **13**.

**(b) …but 11 of those 13 carry an arXiv ID in their `source_uri`** (`2605.30363v1.pdf` and
similar). This is the most important finding in this document, and it inverts the earlier note's
source ranking. See §4.

**(c) Bibliography parsing from PDF text is as hard as it looks.** 12/13 papers have a
`References`/`Bibliography` header, but a two-pattern parse (numbered `[n]` and `Author, I.`)
recovered entries from only **6 of 12** — the rest use formats neither pattern catches. A robust
bibliography parser is a real project with a long tail, and it is the *expensive* way to obtain an
edge that a metadata lookup returns exactly.

**(d) The most-engaged document in the system is not in the corpus.** All 26 rows in
`pdf_annotations` belong to `/reading_list/Advanced Portfolio Management` (11 margin notes, 9
underlines, 6 marks), and there is **no `documents` row with that `source_uri`**. The document he
has engaged with most deeply is invisible to every corpus-side signal. Any design that scores
"engagement" by joining annotations to documents silently ranks it zero.

**(e) The marginalia is a better discovery signal than anything else available.** 16 of the 26
marks now carry transcribed handwriting, and it reads as an explicit reading agenda:

> `read next on alt-data?` · `what is a factor covariance estimator r = Bf + ε` ·
> `what is left to understand? -> research area` · `is a factor like a feature in ML?` ·
> `no momentum in Japan??`

These are in his own words, already about what he wants next, and already attached to the passage
that provoked them. No other signal in the system is this direct.

**(f) `acceptance_log.surface = 'reading'` is already occupied.** `pull_daily.SURFACE_READING`
writes it for the daily page's *re-read* slot; there are 3 rows, all rejections, all with a
`candidate_key` that is a local corpus path (2 coursework, 1 paper). Reusing that surface would
conflate "stop telling me to re-read Mechanical Vibrations" with "don't propose papers like this
one" — two unrelated judgments pooled into one prior. Discovery needs its own surface.

**(g) `open_gap_concepts()` is not usable as a query source as it stands.** It returns 72
concepts. Essentially all sit at `doc_freq = 1`, so `concept_weight` returns 1.00 for every one of
them and **cannot rank among them at all** — the rarity weighting that fixed the re-read slot is
degenerate here, because appearing in one document is the normal case rather than the
distinguishing one. Worse, much of the list is not searchable concepts: `AEX`, `DAX`, `Henry Hub`,
`GDP QoQ`, `55 liquid futures`, `AIS capture`. It also contains `Append-Only Data Storage` *and*
`Append-only data storage` as separate entries — an alias miss. Sent to arXiv as keywords these
return noise, and (§9) leak his project vocabulary for nothing. **Gap-driven proposals need a
filter tier before they can exist at all.**

## 4. Candidate sources, ranked by signal per unit of effort

### 1. Citation edges via OpenAlex, keyed on the arXiv ID — BUILD FIRST

For each of the 11 arXiv-identified papers, `GET /works/arxiv:<id>` returns `referenced_works`,
and each of those resolves to title, authors, year, OA status and a PDF URL when open access. This
yields exactly the thing finding (c) says is expensive to parse, for one HTTP call per paper, with
no parsing and no title-resolution step.

- **Signal:** a citation from a paper he kept is a pre-filtered recommendation, and a work cited by
  **two or more** of his papers is a strong one — that is a co-citation cluster forming around what
  he actually reads.
- **Effort:** low. No key, no auth; polite-pool by putting a contact email in the User-Agent.
- **Dependency weight:** real, and worth accepting *here specifically*, because the alternative is
  the 6/12 parser. It is also cleanly bounded — OpenAlex supplies **edges and metadata only**,
  never content, and if it vanishes the stored proposals and the accept loop are unaffected.
- **Offline complement, not replacement:** a bibliography parse still earns its place for the 2
  non-arXiv papers and for books, where no ID exists. Build it *second*, sized to the tail it
  actually serves.

### 2. Marginalia from `pdf_annotations` — BUILD FIRST (small)

Finding (e) is the argument. Two extractions, both cheap:

- **Citation spans** out of `covered_text` (`Author and Author, YYYY`, `Author et al. YYYY`, bare
  DOIs/arXiv IDs), resolved by title/author search against OpenAlex or Crossref. This is the
  `[Connor and Korajczyk, 2010]` case verbatim.
- **Topic asks** out of the transcribed handwriting — `read next on alt-data?` is a reading
  instruction, not a gap inference. These are the best **book** seeds, and unlike gap concepts they
  arrive pre-filtered by him.

Volume is tiny (26 marks, one book) and that is fine. This channel is not meant to fill a weekly
slot; it is meant to be right.

### 3. arXiv new-listing for currency — DEFER

Free, no key, good metadata, and the natural answer to "what is new in what I care about". Two
reasons to defer: its query terms would come from gap concepts, which finding (g) shows are
currently unusable; and it is the one channel that **sends his vocabulary outward** (§9). Build it
after the gap filter exists, and after there is enough acceptance data to judge whether
keyword-derived proposals are worth their slot.

### 4. OpenAlex related-works / citation-graph recommendations — DEFER

Real discovery — "papers like this" as a graph edge rather than a keyword match. Cheap to add
*once source 1 exists* (same client, same auth story). Deferred only so that it is switched on
against an existing acceptance prior rather than blind.

### On Semantic Scholar

Its recommendations API is good, but it needs a key and has become rate-limit-hostile to
unauthenticated use. OpenAlex covers the same edges without one. Not worth a second dependency —
revisit only if OpenAlex coverage proves thin on quant/finance venues, which is a real risk worth
measuring on run one (its economics coverage is weaker than its CS coverage).

## 5. Schema — migration 0017

Two changes, forward-only. This is agent state: its own tables, nothing in the ingest spine touched
(principles 7–9).

```sql
CREATE TABLE reading_proposals (
    id            INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('paper','book')),
    dedupe_key    TEXT NOT NULL,        -- normalised title + first author; the identity
    external_id   TEXT,                 -- 'arxiv:2605.30363' | 'doi:…' | 'openalex:W…' | 'isbn:…'
    title         TEXT NOT NULL,
    authors       TEXT,
    year          INTEGER,
    url           TEXT,
    abstract      TEXT,
    oa_pdf_url    TEXT,                 -- non-null => the proposal can BE the paper
    why           TEXT NOT NULL,        -- rendered on the page, in full
    why_kind      TEXT NOT NULL         -- the CHANNEL, for the flywheel (§8)
                  CHECK (why_kind IN ('citation','co_citation','annotation','gap',
                                      'arxiv_recent','related_work')),
    evidence_key  TEXT NOT NULL,        -- citing doc source_uri | pdf_annotations.id | gap concept
    score         REAL,
    status        TEXT NOT NULL DEFAULT 'candidate'
                  CHECK (status IN ('candidate','proposed','accepted','ingested',
                                    'rejected','superseded')),
    device_uuid   TEXT,                 -- rmapi document ID, recorded at delivery
    device_folder TEXT,                 -- last observed folder path
    filename      TEXT,                 -- the uniquified name we uploaded
    proposed_at   TEXT,
    resolved_at   TEXT,
    created_at    TEXT NOT NULL,
    source_run    INTEGER REFERENCES agent_runs(id) ON DELETE SET NULL,
    UNIQUE (dedupe_key)
);
CREATE INDEX idx_reading_proposals_status ON reading_proposals(status, kind);
```

`dedupe_key` is the identity, **not** `external_id`: the same work arrives as an arXiv ID from one
citing paper and a DOI from another, and proposing it twice is exactly the suggestion fatigue that
kills this layer (failure mode #7). `why` and `evidence_key` are `NOT NULL` — grounded-or-silent
enforced in the schema rather than in a code path that can be forgotten.

**Widen `acceptance_log.surface` to include `'discovery'`.** SQLite cannot ALTER a CHECK, so this
is a table rebuild in the same shape as 0016's `objects` rebuild, preserving every row. Finding (f)
is the reason. `'reading'` keeps its current meaning — the daily re-read slot.

### Lifecycle

```
candidate ──score, cap──▶ proposed ──moved out of Proposed──▶ accepted ──▶ ingested
    │                        │
    │                        └── TTL in Proposed ──▶ rejected   (weak negative — §8)
    └── already in corpus / already seen ──▶ superseded  (never shown, never counted)
```

`accepted → ingested` is the only transition that writes to the corpus, and it goes through the
ordinary ingest spine with no special-casing: the PDF lands in `vault/incoming/papers/` and the
existing watcher takes it. A proposal is not a corpus document until he moves the file
(propose-never-mutate). Stub pages are agent output and are **never** ingested — the corpus gets
the real book or nothing (invariant 5).

## 6. Where the folder-move watch hooks in — and why the earlier note was over-optimistic

The earlier note said the accept signal "is observable with code that already exists today". That
is nearly true, and wrong in two specific ways:

1. **`build_uuid_index()` excludes `Locus`.** `DEFAULT_EXCLUDED_FOLDERS = ("trash", "Locus",
   "admin")`, mirrored in `CaptureConfig.excluded_folders`. The reading folders live *under*
   `Locus`, so the function meant to observe the move is specifically configured not to see it —
   deliberately, because that exclusion is what stops Loop A ingesting our own pushed pages as if
   they were his handwriting.
2. **It discards the subfolder.** `folder = device_path.lstrip("/").split("/", 1)[0]` keeps only
   the top level, so every proposal reads as `Locus` whether it sits in `Proposed`, `In-Progress`
   or `Finished`. The one field the accept signal is made of is the field that gets thrown away.

Do **not** widen `build_uuid_index` to fix this — that re-exposes Loop A to our own output.

**Instead, a separate and much cheaper watch.** We control the uploaded filename and record the
device uuid at delivery, so the whole signal is one call:

```
rmapi find /Locus/Reading   ->  /Locus/Reading/In-Progress/2026-07-31 Regime Shifts.pdf
                                /Locus/Reading/Proposed/2026-08-02 Covariance Shrinkage.pdf
```

Match each path back to a `reading_proposals` row by `filename`, fall back to `device_uuid` (one
`rmapi stat` **at delivery time**, not a corpus-wide sweep), and compare the subfolder against
`device_folder`. Moved out of `Proposed` ⇒ accepted. That is O(1) rmapi calls per pull instead of
one `stat` per document in the account, and it needs no change to the capture path.

Filenames must be uniquified — date-prefixed, as the daily page learned when `rmapi put` refused a
same-named re-upload (2026-07-30 deploy).

**Hook point:** a new `locus discover --pull`, on the same hourly timer as `locus daily-pull` and
after it. It takes the ingest lock, because accept → ingest writes to the corpus.

## 7. Ranking and caps

**Caps are on the stock, not the flow.** The earlier note said "≤3 stub PDFs a week", which keeps
adding to a folder he has not cleared. The rule instead: **at most 3 papers and at most 1 book may
sit in `Proposed` at any time.** A full folder proposes nothing. That makes "empty is a valid
state" structural rather than aspirational, and it means a fortnight of not reading produces
silence rather than a backlog of six papers. No counts are ever rendered, and nothing reports how
many are unread.

**Score** — deterministic, every term traceable to a stored row:

| term | rationale |
|---|---|
| co-citation multiplicity | cited by ≥2 of his papers; the strongest available signal |
| citing-doc engagement | annotated (`pdf_annotations`) > linked to a blessed `object` > plain corpus doc. Note finding (d): the annotated book contributes nothing here until §12 is decided |
| gap match | title/abstract hits an open gap concept **that survives the filter below** |
| channel prior | from `acceptance_log`, per `why_kind` (§8) |
| recency | papers only, and mild — a 2010 factor-model paper is not stale |

**Hard filters, before scoring:** already in the corpus (match arXiv ID/DOI, then normalised
title); already `proposed`/`rejected`/`superseded`; no resolvable `why`. A candidate that cannot
state its grounding is dropped, not softened — the same rule `surface/critique` applies when it
drops a claim citing evidence it was not given.

**Gap-concept filter** (the finding-(g) fix, required before the gap term *or* arXiv currency can
be used): reuse `link/related.non_topical_names()` — the same predicate `structure/propose` uses,
so the layers keep agreeing on what a concept is — plus a rule that the concept must resolve
through `entity_aliases` to a canonical spanning ≥2 documents, *or* appear in an owner-authored
note. That drops the tickers and the one-off proper nouns, and it will drop most of the 72. Good:
a short list of real gaps is the point.

## 8. What the flywheel learns

With three rejections on the whole `reading` surface today, **per-item learning is not
statistically available, and pretending otherwise would be dishonest.** What *is* available at low
n is per-channel learning:

> "Citation-derived proposals: 4 accepted of 5. arXiv-keyword proposals: 0 of 6."

That is why `why_kind` is a stored column and not a rendered string. Each accept/reject writes
`acceptance_log(surface='discovery', candidate_key=dedupe_key, verdict=…)`, and the channel prior
is a smoothed accept-rate per `why_kind` folded into the score. A channel that never lands goes
quiet; a channel that lands gets more of the slots.

**Three guards:**

- **A TTL rejection is a weak negative.** Three weeks in `Proposed` may mean the proposal was
  wrong, or may mean he was busy. Weight it materially below an explicit reject, and never let TTL
  evidence alone silence a channel.
- **Never to zero.** Every channel keeps a floor, or one bad fortnight permanently kills a source
  that would have worked. The prior tilts slot allocation; it does not gate.
- **Do not delete his files.** The earlier note had proposals "dropped silently" after N days. Mark
  the row `rejected` and leave the file, or move it to `Locus/Reading/Archive`. Automatically
  deleting things off his device to keep our folder tidy is not a trade worth making.

**Accepted proposals also feed `related_documents`:** (citing doc, accepted cited doc) is a
citation-verified related pair, which is exactly what `link/related.acceptance_factors()` already
consumes.

## 9. What leaves the machine

Principle 1 says corpus content never leaves. Fetching public metadata *in* is fine, but this is
the first Locus feature that sends anything outward on a schedule, so the ledger is explicit.

**Sent:**
- **OpenAlex:** an arXiv ID or DOI — a public identifier of a paper he holds. One per lookup.
- **OpenAlex/Crossref title search (marginalia):** an author-year string he underlined, e.g.
  `Connor and Korajczyk 2010`. Public citation text.
- **arXiv (deferred channel):** category codes and keywords.

**Never sent — enforced in the client, not by convention:** chunk or section text, note content,
`covered_text`, transcribed handwriting, `objects.title`, `belief_positions.stance`, `source_uri`
paths, or gap-concept lists as free text.

The aggregate leak is real even so: the set of IDs queried reveals his reading list to whoever
serves the API. That is an acceptable trade for public paper metadata, and worth stating plainly
rather than eliding.

**This is the strongest argument for deferring the arXiv keyword channel.** Sending an ID says
"someone has this paper". Sending `market regime detection, walk-forward cross-validation,
Optibook` says what he is building. If that channel is ever built, it may send only terms that are
publicly attested concepts *and* pass the §7 filter — never object titles, never raw gap strings.

## 10. Build order

**First — the accept loop, before any source.** Migration 0017; the stub / real-PDF renderer
(`reading/md2pdf` + `deliver_remarkable.deliver_pdf`, date-uniquified); `locus discover --pull`
with the `rmapi find` watch; accept → `vault/incoming/papers/` → existing watcher; the
`acceptance_log` write. Validate end to end with **one hand-seeded proposal row** and a real folder
move on the device.

The sequencing is deliberate: the accept loop is what makes every later source *safe* to switch on,
because nothing reaches the corpus without a deliberate physical gesture. Building sources first
yields candidates that cannot be acted on, and a temptation to shortcut the gate.

**Second — the citation channel.** A bounded OpenAlex client (responses cached in `pass_cache` by
`external_id`, so a re-run costs no requests), `referenced_works` for the 11 arXiv papers,
co-citation counting, scoring, the ≤3 stock cap. This is the channel that should produce the first
real proposal.

**Third — marginalia.** Citation spans and topic asks out of `pdf_annotations`; the book path and
its single-slot cap. Small, and the highest-precision thing here.

**Deferred, in order:** the bibliography parser for the non-arXiv tail; the gap-concept filter tier
(§7) — which is *also* what the concept-promotion work in CLAUDE.md §15 needs, so it may well
arrive from that direction; arXiv currency; OpenAlex related-works.

**Explicitly not in Phase 4: any model pass.** Every ranking term above is a join or an arithmetic
count. A model could write a nicer *why*, but the why is a citation and a gap — already true
without one. `claude -p` in this loop would add spend, latency and a hallucination surface to a
pipeline that needs none of the three.

## 11. The eval consequence

CLAUDE.md §11 says labels grow with the corpus, and this is the first feature that grows the corpus
*automatically*. Two things follow.

**Accepted proposals are free eval labels.** When a proposal is accepted we already know why it was
proposed — the gap concept it matched, or the paper that cited it — and the target's `source_uri`
is known at ingest. That is a query→target pair for nothing. Write them to a **suggestion file**,
not into `retrieval_eval.py`: §11 requires every label be verified live, and the labelled set is
not fully deterministic (multi-query expansion rephrases through local qwen, so borderline targets
move between runs). Auto-inserting unverified labels would corrupt the one measurement that tells
us whether any of this worked. Same for `links_recall`: (citing doc, accepted doc) is a strong
candidate pair — proposed for verification, not merged.

**Expect the numbers to fall, and do not treat that as a regression.** Adding quant density adds
competitors for the top-k slots on every existing quant query. recall@k 0.983 over 60 queries was
re-baselined on a set built largely from verified-passing labels; it will move. The honest gate is
not "recall@k held" but **"recall@k held on a label set that grew with the corpus"** — a
frozen-label recall number rising while the corpus triples would mean nothing at all.

Concretely: after the first ~10 accepted papers, re-curate before reading the numbers, and expect
that re-baselining to be the real work of the session that follows.

## 12. Open questions worth deciding before building

- **OpenAlex coverage of quant/finance.** Its CS coverage is good; economics and finance are
  thinner. Measure on run one — if `referenced_works` comes back sparse for the finance papers, the
  bibliography parser stops being a tail case and moves up the order.
- **Should the annotated book enter the corpus?** Finding (d): the most-engaged document in the
  system is invisible to every corpus-side signal. It is a third-party book, which is why it was
  kept out. But `pdf_annotations` already stores its marked passages, and its bibliography is the
  best book-discovery seed available. Ingesting *his marks and the passages they cover* without
  ingesting the book is the likely answer — and it is a decision, not an implementation detail.
- **A proposal moved to `Finished` without ever being opened** is currently indistinguishable from
  a real read. Probably acceptable; worth not pretending otherwise.


## What measurement changed

Written after building it. Each of these was a considered decision that the evidence reversed.

**Category browsing was the wrong mechanism, and it was the whole mechanism.** Harvesting recent
listings caps the pool at a rolling window, and METHODS ARE OLD — the canonical treatment of a
technique is usually years back. A relevance search for `regime switching AND hidden Markov`
returns work from 2008, 2014 and 2020, none of which any recency browse could reach. Browse is
now retired (`[discovery].browse_categories = false`), kept as a knob rather than deleted.

**arXiv alone is not enough.** It is preprints, skewed to CS/physics/maths. A search for
`Kalman filter AND trajectory interpolation` — a problem he has written down — returned ZERO
results there, because that work is published elsewhere. OpenAlex added journals, books and
chapters, and brought citation counts, which gave the quality prior the design never had. It
immediately surfaced Harris's *Trading and Exchanges* and Christoffersen's VaR backtesting paper.

**The profiles were the bottleneck before the ranking was.** `regime-ml` was represented by 289
characters against 34,856 available — its section summaries, `result`, `limitations` and 263
named methods all unused. One vector per project matched relevant work by luck. Facets fixed it.

**Concepts he MARKED WHILE READING are the best query source**, better than anything derived from
his code, and the design did not mention them. They now lead the search rotation.

**Two interleave bugs, the same mistake twice.** Concatenating sources meant a truncated budget
took only the first: 79 papers harvested and ZERO project or gap concepts ever searched. The
ranking had the identical flaw. An ordering has to be balanced at every prefix, not just at the
end.

**A local judge is a filter, not a ranker.** Asked yes/no it rejected all 14 candidates including
the two best; asked for a 1-5 score it binned exactly the junk. It is wired as a floor only.

**The familiarity term was premature.** Swept, 1.0 destroyed the ranking by burying the two best
papers. At 205 documents "he already has this" is rarely true, so it now runs at 0.25 as a
tiebreaker and should rise as density grows.

**Coursework concepts are kept deliberately.** `eigenvector` and `frequency response` look like
noise and are not: the link between eigenvectors in factor models and in modal analysis is exactly
the cross-domain transfer this engine exists for. The owner overruled a proposal to filter them,
and an AIAA paper on eigenvector rates of change now appears legitimately.

**Accepting a paper stopped the loop.** `watch.scan` only revisits `status='proposed'`, so once a
paper was ingested nothing watched it again — he could read and annotate it and none of that
reached the corpus. `reading/sweep.py` (migration 0022) closes it, guarded by a stroke fingerprint.

## Operations

    locus discover --harvest --profiles   # search arXiv + OpenAlex for your concepts, embed
    locus discover --rank --top 10        # what it would propose, and why
    locus discover --propose --push       # fill free slots and put the PDFs on the tablet
    locus discover --pull                 # observe moves, ingest accepted, read back your marks

Timers (`deploy/systemd/`): `locus-discover-pull` hourly (free, local), `locus-discover-harvest`
weekly (network + GPU). `locus status` reports proposals in flight, pool size and marks read back.


## Addendum — what shipped after the section above was written

The "What measurement changed" notes were written mid-build and eight commits landed afterwards.
Consolidated, so this document matches the code:

**Browse retired, search added.** `[discovery].browse_categories=false`. `--harvest` now means
"search arXiv and OpenAlex for my concepts". Queries are interleaved across `marked` / `reading` /
`project` / `gap` so a truncated request budget still covers every source — concatenating them
meant 79 papers were harvested from ONE book and no project concept was ever searched.

**A `marked` tier, because the claim was false.** Proposals said "a concept you marked while
reading X" about concepts drawn from anywhere in an annotated document — 1,212 entities against 26
actual marks. Now only a concept occurring inside a stroke's covered text (or the handwriting
beside it) makes that claim; the rest say "a concept from X, which you annotated".

**OpenAlex**, with the phrase QUOTED. Unquoted, `title_and_abstract.search` ANDs the words
anywhere: "Information Ratio" returned 385,741 works led by Shannon's *Elements of Information
Theory*; quoted, 1,071 led by "The Information Ratio". Precision first, loose as the fallback.

**Citation prior centred on the pool median.** Raw log-citations scored UNKNOWN as 0.0 while the
median known count (601) was worth +0.42 — so every arXiv preprint sat behind every journal
article for its source alone. Centred, unknown maps to the middle of the field.

**The judge is a filter, never a ranker.** Asked yes/no it rejected all 14 candidates including
the two best; asked for a 1-5 score it binned exactly the junk. It drops the floor and leaves the
cross-encoder's ordering alone.

**Step 3 (citation mining) is built and idle.** OpenAlex holds no `referenced_works` for arXiv
preprints, and every identifiable document in the corpus is one. It activates on the first
accepted journal article.

**`reading/sweep.py` closes the loop.** Accepting a paper used to end the system's interest in it,
so anything written afterwards was never captured. Every delivered reading is now checked for new
ink hourly, guarded by `rmdoc.ink_hash`.

### Operational faults found by running it

- `rmapi find` renders paths relative to the PARENT of what it searched and without a leading
  slash. Prefix-matching the root matched nothing, so ten live papers read as "deleted"; and
  passing that rendering to `rmapi get` fails. Take the file's immediate parent as the folder and
  reconstruct absolute paths.
- `fetch_rmdoc` inherited stdin and a 1800s timeout, so one hourly run blocked for 34 minutes
  HOLDING THE INGEST LOCK. stdin is DEVNULL and the sweep passes 180s.
- systemd's user PATH excludes `~/.local/bin`, where `uv`, `rmapi` and `claude` all live. Every
  unit now declares PATH; without it `locus-maintain` failed six consecutive nights, silently
  taking `locus link` and `locus structure` with it.

### Known open

Currency: retiring browse removed the only mechanism that surfaced NEW work. The fix is cheap and
additive — `from_publication_date` is just another filter on the same relevance search, so a
date-bounded pass can be its own channel without touching the existing one.

A bibliography parser is now the ONLY route to the book's references and to any preprint's, since
OpenAlex indexes neither. Its priority rose even though its difficulty did not: a parser aimed at
one book faces one citation style.

Eval labels have NOT grown with the corpus (§11) — still 60 queries / 17 pairs, none covering the
four papers accepted on 2026-08-01.
