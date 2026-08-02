# Locus — daily-use refinement: device, page, reporting, marks→ideas

**Status:** approved 2026-08-02. **Step 1 (§1, the device overhaul) is IMPLEMENTED**; steps 2–5
are not started. Settled in a nine-round requirements interview — where the owner's answer
overruled a proposal of mine, §8 records which and why, so the reasoning is not lost.

## Context

Three loops work (memory, capture, discovery) but they don't form a system. Measured
2026-08-01: `compose_daily.py` has zero references to `reading_proposals`, so "Read next"
offers corpus re-reads (the Optibook Python reference — a year-old competition manual) while
ten real papers sit in `Reading/Proposed`; nothing reports what ran overnight; `locus-maintain`
failed six consecutive nights unnoticed; 26 marks in `pdf_annotations` have never become
anything the owner can develop; and the device tree has a split reading identity
(`/reading_list` vs `/Locus/Reading`), daily pages accumulating loose forever, and folder names
that silently drive ingest categories.

The interview (nine rounds, 2026-08-01/02) settled the shape. The load-bearing answers:

- **The page is four physical pages**, one section each, laid out to look good. The cap is
  "what fits", not a number. Best items fit; the rest wait.
- **No more near-duplicate sections.** Marks, ideas, open threads and connections are *one*
  thing — stuff of his to develop — and belong in one section.
- **"Never linked" is a fact about the database, not a reason to care.** Every item must say
  how it bears on something he is doing, and name the code/passage.
- **Approving moves off the tablet** into a Textual TUI, and **no decision may ever appear on
  both surfaces**: thinking on the page, approving in the TUI.
- **`maturity` is provenance, not quality.** A lecturer's aside beats a neat textbook summary,
  because the aside exists nowhere else. It must never become a ranking penalty.
- **A connection he keeps must have a fate**: it becomes an `idea` thread, and a corpus note
  once he has written on it.

---

## 1. Device layout

### Target tree

```
/Daily/                       Locus writes.  An INBOX: unmarked pages stay loose here.
    2026-08-02                  built today
    2026-07-31                  still loose = you haven't been through it
    2026-07/                    archived only once a page has ink on it
/Reading/                     both write.  One lifecycle for everything he reads.
    Proposed/                   Locus only.  Moving OUT = accept (unchanged signal)
    In-Progress/                his own books/PDFs drop straight in here
    Finished/                   final sweep + citation harvest
/Notes/                       he writes.  Locus READS (Loop A capture -> ingest)
    engineering/                -> coursework
    quantum_ml/                 -> note      (research internship, not study material)
    brevan_howard/              -> note
    projects/                   -> project
    careers/                    -> career
    rough_notes/                -> note
/Admin/                       excluded from everything
/trash/                       excluded
```

Reads vs writes is now structural: **`/Daily` and `/Reading/Proposed` are Locus-owned;
`/Notes` is his and Locus only ingests from it; `/Admin` and `/trash` are invisible.**

**What "Locus writes, never ingested" means for `/Daily`.** The page *document* never becomes a
corpus row — agent prose must not re-enter the corpus as his (invariant 5). His **ink on it is
fully processed**: `agent/pull_daily.py` extracts each region and routes it into agent state
(idea/question objects, SM-2 grades, acceptance verdicts, developed threads). A thread he
develops reaches the corpus by the proper route — `agent/promote.py` writes it to
`vault/notes/threads/`, `notes_sync` ingests it, and only the fields carrying his `_owner_edits`
marker are written. So the container is excluded; his words are not.

### The coupling this changes

`capture/remarkable.py:38` (`DEFAULT_FOLDER_CATEGORY`) and `build_uuid_index()` key on the
**top-level** folder (`device_path.lstrip("/").split("/", 1)[0]`). Under the new tree every
note's top level is `Notes`, which would collapse all categories to the default. Change the key
to *the first segment beneath the notes root* (`Notes/engineering` → `engineering`), keep
`DEFAULT_FOLDER_CATEGORY` as the map, and set the new values in `[capture].folder_category`
(mirrored into `config.example.toml`). `excluded_folders` becomes `("trash", "Daily", "Admin")`
— `reading_list` disappears with the merge, and `Reading` is excluded from Loop A but watched by
`reading/watch.py` and `reading/sweep.py`, which is the existing division of labour.

`reading/deliver.py:37` `DEFAULT_ROOT = "Locus/Reading"` → `"Reading"`; `reading/watch.py:46`
likewise. `[reading].target_folder = "Locus"` → `"Daily"`.

### Migration — `locus device-migrate`

New CLI verb. Dry-run by default, `--apply` to execute, all over the existing rmapi runner
(no second transport). It prints every move as `old path -> new path` and refuses to run if any
target name collides.

**The plan is item-level, not folder-level, and you edit it before it runs.** The dry-run writes
`vault/device-migration.toml` listing every document with its proposed destination; you change
any line and re-run. Folder rules generate the defaults, per-item overrides win. This exists
because `/reading_list` is already not homogeneous — it holds the annotated *Advanced Portfolio
Management* PDF **and** a handwritten reMarkable notebook (a reading list someone gave you),
which are two different things and must go two different places.

Order: pull a **full local snapshot** of every document first (`rmapi get` into
`vault/backups/device-<date>/`), then `mkdir` the new tree, then move. You chose "script moves
everything" over the snapshot variant — I am adding the snapshot anyway, because rmapi moves at
scale run against your only copy and the snapshot costs one command and some disk. Say the word
and I will drop it.

Mapping applied:

| today | becomes |
|---|---|
| `/Locus/Reading/{Proposed,In-Progress,Finished}` | `/Reading/{...}` |
| `/reading_list/Advanced Portfolio Management` (and any other PDF) | `/Reading/In-Progress/` |
| `/reading_list/<the handwritten reading-list notebook>` | `/Notes/brevan_howard/` — a note, not a book |
| `/trash/reading_list` | left in trash |
| `/Locus/daily-2026-08-01` and older | `/Daily/2026-08/` |
| `/Locus/pour-runbook` | `/Notes/projects/` |
| `/engineering`, `/quantum_ml`, `/brevan_howard`, `/careers`, `/rough_notes` | `/Notes/<same>` |
| `/projects/oqts` | `/Notes/projects/oqts` |
| `/admin` | `/Admin` |

Merging `/reading_list` into `/Reading` is what finally lets the annotation sweep see the
most-annotated document in the system — finding (d) of the discovery plan, where 26 marks belong
to a document no corpus-side signal can see.

---

## 2. The daily page

A four-page PDF, one section per page. `compose_daily.py` keeps its **aggregate-only** contract:
every reason printed on the page is *written and stored beforehand* (overnight, or at proposal
time), so the page still renders identically whether or not last night's model runs succeeded.

### Page 1 — Read

**What this page is for:** it is the *why* layer over the shelf. The PDFs are already on the
device in `Reading/Proposed` — this page is how you decide which one to start, without opening
each to find out. Three things on it: why each proposal was chosen and what it bears on; the
state of what you are already reading; and any re-read that answers something you marked as not
understood. It is a decision aid, not a second inbox.

Sourced from `reading_proposals` (which `compose_daily` has never referenced) plus in-flight
state from `reading/watch.py`, plus targeted re-reads.

```
D1  Ledoit-Wolf, "Honey, I Shrunk the Covariance Matrix"        [Proposed]
    regime-ml estimates a 55x55 covariance from 250 days
    (features/factor_cov.py:88). This is the standard fix for that
    ratio, and you have never used it.
        matched: factor covariance · shrinkage    cited by 2 papers you kept

D2  Mechanical Vibrations §4.2                                  [re-read]
    you marked p.88 "don't follow this" on 24 Jul — §4.2 derives
    the step that page skips.

In progress:  Advanced Portfolio Management (p.114, last ink 31 Jul)
              Christoffersen, Backtesting VaR (no ink yet, 6 days)
Proposed:     6 waiting, oldest 3 weeks
```

- The prose reason is written **once, at proposal time**, by a `claude -p` pass that reads the
  abstract against the project/thread that matched it, and stored on the row. Beneath it, the
  deterministic grounding it must be checkable against.
- **True counts are shown** — your call, and it deliberately drops the no-guilt-metrics rule for
  reading only.
- **Queue policy: full by displacement, not accumulation.** The cap is on the stock in
  `Proposed` (already true today), so "always full" cannot make the shelf grow — refill happens
  when something leaves. The residual risk is *staleness*, not volume, and two things handle it:
  (a) a clearly-better candidate **evicts the weakest item on the shelf** rather than waiting for
  a slot, so nothing good is ever blocked by a full folder — the evicted proposal moves to
  `Reading/Archive` and is marked `superseded`, never deleted (§8 of the discovery plan: do not
  delete his files); (b) a proposal still unaccepted after **7 days gets its `why` rewritten**
  against your current threads, so nothing on the shelf is ever both stale and unexplained.
- Targeted re-reads appear **only** when they answer a passage you marked as not understood.
  Never a generic gap-closer — that is the rule that killed this section.

### Page 2 — Think

One page, one action vocabulary (react / develop), but **three labelled subsections that say
where each item came from** — the confusion you objected to was sections that all felt the same
without saying why they differed, not the fact of grouping. `build_marks`,
`build_open_threads` and `build_connections` keep their identities as three builders feeding one
ranked page with a shared item type and one anchor series.

```
From your reading
  T1  "factors are just features"
      your margin note — Advanced Portfolio Mgmt p.114, 24 Jul
      ______________________________________________________

Still open
  T2  "what is a factor covariance estimator?"      raised 29 Jul
      > 28 Jul you wrote: "...but features are learned"
      Answer, from your corpus: r = Bf + ε, where B is ...
          — Advanced Portfolio Mgmt p.112; regime-ml features/factor_cov.py
      ______________________________________________________

Connections found
  T3  Christoffersen §3 bears on T2: loadings are ESTIMATED from
      returns, not learned by optimisation. regime-ml already
      implements the estimator he critiques (features/factor_cov.py:88).
      [ ] keep    __________________________________________
```

The subsection names describe **provenance**, which is the thing that was missing: *From your
reading* is ink you made in a book, *Still open* is a thread you own, *Connections found* is the
only agent-originated one — and it is the only one carrying a `[ ] keep`, which makes the tick
mean exactly one thing on the whole page. A subsection with nothing in it is omitted.

- Ticking a connection creates an `idea` object linked to the project **and** the paper; it
  returns here for development and is promoted to `vault/notes/threads/` by the existing
  `agent/promote.py` once you have written on it. (Your "both idea and note".)
- A question you wrote yesterday comes back **answered** — grounded and cited from the corpus —
  and if the corpus cannot answer it, it is also injected as a discovery query
  (`discover/queries.py` gains a `question` source alongside `marked`/`reading`/`project`/`gap`).

### Page 3 — Recall

Question, ruled space, `[ ] knew it`. No grading call, no answer on the page.

### Page 4 — Answers + open space

Answers printed small at the foot; the rest is blank for anything on your mind. Free writing
becomes an `idea` thread now, and a corpus note once developed (same path as everything else).

### Status line

Foot of page 1, always present, one line: what it produced, and loudly what is broken.

```
overnight: 3 papers found, 1 proposed, 12 marks read back · all timers healthy
overnight: nothing new · MAINTENANCE HAS FAILED 6 NIGHTS
```

### Cadence — build daily, never repeat, archive on interaction

Three rules, replacing the earlier "don't rebuild until read" (which you rejected, rightly: it
makes a *daily* page not daily, and misses compound).

1. **A page is built every morning at 05:30 regardless.** No gating on whether you read the last
   one.
2. **No item ever appears on two pages.** A new `daily_shown (item_key, page_date)` table records
   every item as it is rendered, and every builder excludes anything already shown. `item_key` is
   the stable key already used for anchors (`source_uri`, object id, review item id) — never a
   title, which `retitle` has broken before. Skipping a day therefore *loses nothing and repeats
   nothing*: yesterday's items are still on yesterday's page, sitting in the inbox.
   - **The one exception is the Read page**, and it is narrow: a proposal still sitting in
     `Proposed` may reappear **only when its `why` has been rewritten** (the 7-day refresh
     below), so a repeat always carries new text. Otherwise an unaccepted proposal is carried by
     the one-line `Proposed:` summary, not as a full item.
3. **`/Daily` is an inbox.** A page stays loose at the root until it has ink on it; once
   `daily-pull` finds any stroke, `daily_pages.read_at` is set and the page is moved to
   `/Daily/YYYY-MM/`. Opening the folder therefore shows exactly what you have not been through
   — a true state, not a count, and no separate "unread" bookkeeping to keep honest.

---

## 3. Decisions TUI — `locus decide`

Textual app (new dependency, behind an extra). Sections by type: proposed objects, "why did you
stop reading X" (a paper untouched in `In-Progress` for 14 days), ambiguous mark intents,
duplicate-object merges. Everything pending in one pass, cleared fast.

**The disjointness invariant:** a pending decision has exactly one home. Implement as a single
`pending_decisions(conn, surface=...)` query with `surface in ('page','tui')` assigned per
decision kind, both surfaces reading it, and a **test that asserts the two result sets never
intersect**. `compose_daily.build_blessings` is deleted; the `B*` anchor and
`pull_daily._route_blessing` go with it.

The abandonment reason must *do* something: it writes `acceptance_log(surface='discovery')` with
the stated reason, folding into the per-`why_kind` channel prior that already tunes what gets
proposed (`docs/reading-discovery-plan.md` §8).

`locus review` is already taken by SM-2, hence `decide`.

---

## 4. A mark becomes an idea becomes a note

```
underline + margin note
  -> reading/sweep.py (hourly, stroke-fingerprint guarded)
  -> page untouched 12h            <- your settle window
  -> capture/mark_text.py transcribes the ink (already built)
  -> NEW: intent pass (claude -p) -> important | not_understood | idea
       idea            -> `idea` object, linked to the passage AND the project it names
                          -> Think page -> development -> locus promote -> vault/notes -> corpus
       not_understood  -> a targeted re-read on the Read page + a discovery query
       important       -> retrieval + recall candidate only; never pushed at you
       low confidence  -> TUI, for you to correct
```

`pdf_annotations` gains `intent`, `intent_confidence`, `object_id` (the last already exists and
is already the has-been-dealt-with flag `build_marks` reads).

**Note provenance** is inferred by the same kind of pass over the transcription — lecture / talk
/ reading / mine — stored on the document and displayed when a note resurfaces ("you heard this
from a speaker, 12 Jun"). It is **displayed, not ranked**: a ranking knob exists in config and
defaults off. `rough_penalty` stays at 0.0.

**Provenance escalates to the TUI if the model proves unreliable.** Ship it as inference with a
confidence score; low confidence routes to `locus decide` for you to set. If accuracy turns out
poor across the board, `[capture].provenance_always_confirm = true` makes *every* guess a TUI
item — the model still guesses (so it is one tick, not a menu), but nothing is stored unblessed.
That is a config flip, not a rebuild, so the first month's real accuracy decides it rather than
either of us guessing now.

---

## 5. Reporting and failure

- `agent_runs` already journals every run. Add a `locus status --since yesterday` block and the
  one-line summary the page prints, both reading it.
- **Failure detection:** every `deploy/systemd/locus-*.service` gains
  `OnFailure=locus-failure@%n.service`, a tiny unit that records the failure in a new
  `timer_failures` table. The status line reads consecutive-failure counts. This is what would
  have caught `locus-maintain` on night one instead of night six.
- `locus status` grows the same block so it is visible at the laptop too.

---

## 6. Schema (migration 0023, forward-only; head is 0022)

- `daily_pages`: `read_at` (set on first ink; drives the archive move)
- new `daily_shown` (item_key, kind, page_date) — the no-repeat ledger
- `pdf_annotations`: `intent`, `intent_confidence`
- `reading_proposals`: `why_long` (model-written at proposal time), `why_written_at` (drives the
  7-day rewrite); `why` keeps its deterministic, NOT NULL grounding role
- `documents`: `provenance` (`lecture|talk|reading|mine|null`)
- new `timer_failures` (unit, failed_at, consecutive)
- new `question_answers` (question object id, answer text, evidence keys, answered_at) — so an
  answer is stored with the evidence it cited and can be dropped if it cites anything it was not
  given, the same rule `surface/critique.py` applies

Config additions mirrored into `config.example.toml`: `[capture].folder_category` (new keys),
`[capture].excluded_folders`, `[reading].target_folder`, `[reading].root`, `[daily]`
(page geometry per section, settle hours), `[discovery]` top-up behaviour.

---

## 7. Build order

**First — the device overhaul (§1).** Everything else addresses folders, so it goes first, and
it is the cheapest to verify. `device-migrate` with snapshot + dry-run, the folder→category
re-key, the constant changes in `deliver.py`/`watch.py`, the `/reading_list` merge. Ends with
the sweep finally seeing *Advanced Portfolio Management*.

**Second — the page rebuild (§2).** Four pages, the Read page wired to `reading_proposals`, the
Think page with its three provenance subsections, the status line, and the cadence rules
(`daily_shown` no-repeat + archive-on-interaction). This is the surface you touch daily and you
named it joint-first.

**Third — decisions TUI + the disjointness invariant (§3).** Small, and it is what lets
blessings leave the page cleanly.

**Fourth — mark intent and the idea path (§4).** The one genuinely new model pass, and the one
that turns capture from write-only into the thing that generates ideas.

**Fifth — reporting (§5).** Cheap, and the status line placeholder from step two becomes real.

**Deferred:** answering questions from the corpus (Think page T3) until intents land — the
question channel is worth little until marks are producing questions. The provenance ranking
knob stays off until there is evidence. Note↔note surfacing, the concept-promotion tier and the
stronger rough-note summary pass (CLAUDE.md §15 "Next") are untouched by this and stay queued.

---

## 8. Where I think you are wrong

1. **On the full queue I was half wrong, and the correction matters.** The cap is on the *stock*
   in `Proposed`, so keeping it full cannot make it grow — my "read-next died because something
   had to fill it" analogy doesn't transfer, because that section had no cap and no accept
   gesture. The real risk is a shelf of six things that were relevant a month ago. Hence
   displacement + the 7-day why-rewrite (§2), which is what I now recommend rather than any form
   of throttling. **Nothing good is ever blocked**: an excellent candidate takes a slot from the
   weakest occupant instead of queueing behind it.
2. **The `why` rewrite is agreed at 7 days**, down from the 14 I first proposed.
3. ~~Refresh after three days~~ — withdrawn. Your objection is better than my proposal: a page
   that appears on an irregular cadence is not a daily page, and the actual thing to prevent is
   *seeing the same item twice*. The `daily_shown` ledger prevents that directly, which makes
   the gating unnecessary. Design updated to build daily, never repeat, archive on interaction.
4. **Provenance inference is a model judgment on noisy OCR** — I still think the folder is the
   better prior. We do it your way and measure; `provenance_always_confirm` is the escape hatch
   and costs one config line, so this resolves on evidence in a few weeks rather than by
   argument now.
5. **Cost is fine, but be precise about which budget.** The four new passes (mark intent,
   connection reasons, proposal whys, question answering) all go through `agent/claude.py` —
   `claude -p`, env-scrubbed, subscription, which is the headroom you said you have. Two things
   already in the system do **not**: `capture/transcribe.py` uses the metered SDK
   (`transcribe_model = "claude-sonnet-5"`), and `[ingest].pass_routing` bills
   `ANTHROPIC_API_KEY`. Those are unchanged by this plan, but they are where a surprising bill
   would come from, so `locus status` should report both ledgers separately rather than one
   number.

---

## 9. Verification

- `locus device-migrate` dry-run reviewed by eye before `--apply`; `rmapi find /` afterwards
  compared against the target tree; snapshot retained until you confirm.
- Capture: write one note in `/Notes/engineering` and one in `/Notes/quantum_ml`, run
  `locus capture-sync`, assert categories are `coursework` and `note` respectively — this is the
  regression the re-key could silently cause, so it is the first thing checked.
- `locus device-migrate` plan file: confirm the handwritten reading-list notebook is retargeted
  to `/Notes/brevan_howard` and the *Advanced Portfolio Management* PDF to `/Reading/In-Progress`
  — the per-item override is the whole reason the file exists, so exercise it.
- Page: `locus daily --dry-run` renders the 4-page PDF locally; inspect the PDF, then deliver and
  read it on the device. Annotate it, `locus daily-pull`, confirm each region routed to the right
  place and that nothing routed to a blessing (that surface is gone).
- **No-repeat:** build three consecutive days against a static DB and assert the three pages
  share no `item_key` — except a Read item whose `why_written_at` crossed 7 days, which must
  appear with *changed text*. This is the rule most likely to regress silently.
- **Inbox behaviour:** an unmarked page stays loose in `/Daily`; annotate it, pull, and confirm
  it moves to `/Daily/YYYY-MM/` and that `read_at` is set once and not overwritten on re-pull.
- **Displacement:** seed a full `Proposed` shelf and a high-scoring candidate; assert the weakest
  occupant moves to `Reading/Archive` and is marked `superseded`, the file is not deleted, and
  the new proposal is delivered.
- Disjointness: a test asserting `pending_decisions(surface='page')` and `(surface='tui')` never
  intersect, plus `locus decide` showing exactly the items the page does not.
- Marks: run the intent pass over the 26 existing marks on *Advanced Portfolio Management*,
  eyeball the three-way split before wiring it to object creation.
- Failure path: `systemctl start locus-maintain` with a deliberately broken unit, confirm the
  `timer_failures` row and the loud status line.
- Regression suites: `uv run pytest tests/test_compose_daily.py tests/test_pull_daily.py
  tests/test_remarkable.py tests/test_reading_*.py` while iterating (not the full ~830).
  Then `locus eval --suite retrieval` + `locus audit` once, since ingest categories move.
- **Restart `locus mcp`** after the surface changes (stale servers run old code).
