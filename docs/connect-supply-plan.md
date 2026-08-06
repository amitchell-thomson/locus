# CONNECT — supply, quality, depth: measurement and experiment plan

> **2026-08-06 addendum — experiments ran; the redesign below is implemented.**
> `scripts/analysis/connect_exp1.py` A/B'd 12 live pairs × {current system, deep-context
> Haiku, deep-context Sonnet}, judged by hand; `connect_attest_gate.py` measured the
> attestation gate; `connect_e2e.py` verified the shipped path end-to-end with live calls
> on a snapshot DB. Results that decided the design:
>
> - **Deep context wins.** With entity-anchored sections + README/project-object body for
>   repos (2800 chars/side), prose names the owner's actual functions and constants
>   (`run_backtest_mvo`, the flat 0.02 edge threshold, `question_8b()`); the old 1400-char
>   LIKE-matched context produced generalities.
> - **Sonnet is a grounding property, not a fluency one.** On the junk probe (spurious
>   `Markov model`), Haiku bluffed twice — "inspired by Le Châtelier's Principle" — while
>   Sonnet answered NO_CONNECTION. `[agent].connect_model = "sonnet"` (~4 calls/night).
> - **The attestation gate separates junk from good on all available labels**: concept
>   string in ≥1 side's narrative passes 11/12 written notes and rejects `while loop` and
>   the `Markov model` probe (both attested on ZERO sides).
> - **Implemented** (same commit): project arm (repos as sources — the owner's top want),
>   capture arm uncapped 12→64, attestation gate (`connect.attested` in gate_log),
>   title-nesting guard, model-picked concept from the full list verified via a final
>   `CONCEPT:` line, NO_CONNECTION stored as an empty verdict so pairs are never re-paid,
>   bounded shorter-retry replacing the mid-word `prose[:400]` clip.
> - **Invariant §8 held by measurement, not hope**: three deep notes are taller than the
>   page three ~300-char notes fit. Re-measured by rendering the Connect section alone
>   (`render_connect_isolated.py`): 3×340 and challenge+2×380 fit one page; 3×500 spills.
>   `build_connections` therefore budgets characters (`_CONNECT_CHAR_BUDGET = 1000`,
>   challenge headline included) — a note too tall for what remains WAITS un-shown at full
>   depth rather than being clipped. Whole-document page counts were a confounded metric
>   (clearing `daily_shown` for a worst-case test changes the other sections too); the
>   isolated render is the honest one.
> - **Not done, deliberately**: pair-level acceptance keys (the flywheel consumer
>   `related.acceptance_factors` resolves keys as document URIs and silently drops
>   unknown ones — changing the producer alone would darken the flywheel); paper↔paper
>   arm (neither side is his; not in his stated priorities).

Measured 2026-08-06 against a `VACUUM INTO` snapshot of the live `vault/locus.db`
(225 docs, HEAD `2eda669`). Every number below traces to one of:

| script | what it measures |
| --- | --- |
| `scripts/analysis/connect_supply.py` | what has ever been written / shown; today's live pool |
| `scripts/analysis/connect_pool.py` | the true pool ceiling from the substrate; reachability of the walk |
| `scripts/analysis/connect_eligible.py` | which pool pairs the current rules can *ever* reach |
| `scripts/analysis/connect_quality.py` | per-pair discriminators for the 12 written notes |
| `scripts/analysis/connect_depth.py` | how much text each side gets vs what exists |
| `scripts/analysis/connect_projection.py` | supply × quality, per candidate source arm |

No code in `locus/` was changed. Nothing here is implemented.

---

## 0. The headline

**The binding constraint is not the gate and not ranking. It is structural eligibility.**

Of the **3,784** document pairs that share a canonical concept passing the current
`_substantive_shared` gate, only **431** are reachable by the current source rules —
**3,353 can never be offered no matter how anything is ranked or thresholded.**

```
pool pairs carrying a qualifying canonical : 3784
  eligible via the bridge arm              :  379
  eligible via the capture arm             :   52
  STRUCTURALLY UNREACHABLE                 : 3353
```

The reachable set is also nearly consumed: **12 connection notes have ever been written,
8 have been shown**, and today's live `connection_candidates` returns **8 candidates, 7
with prose** — of which 8 are already in `daily_shown`. Three seats a day against a walk
that emits at most one pair per source is why it feels starved.

---

## 1. Supply

### 1.1 The concept funnel is healthy; the walk is not

```
canonicals in canon_docs           12528
  spanning >= 2 documents           2146
  length >= 6                       1987
  multi-word                        1615
  not non-topical                   1615   <-- rejects ZERO (see 1.4)
  in TEACHABLE_TYPES  (QUALIFYING)  1421
                       -> induces   3784 document pairs
```

1,421 qualifying concepts is not a supply problem. What consumes them is the walk:

```
capture sources (12 most recent owner-authored)      12
bridge sources  (paper|project, source_type != code) 24     [docstring claims 34]
pairs the top-5 walk touches at all                 140
  ... carrying a qualifying canonical               100
pairs in the pool the walk never sees              3684
```

Raising `top_n` alone is weak medicine: `top_n=40` still only reaches 501 qualifying
pairs, at 8× the per-source cost, and each source still emits **one** pair
(`break` in `connection_candidates.collect`).

### 1.2 Where the unreachable supply lives

Pool pairs by class, and how many the current rules can reach:

| class | in pool | eligible now | note |
| --- | ---: | ---: | --- |
| coursework ↔ coursework | 2739 | 0 | correctly excluded — not his material |
| coursework ↔ paper | 366 | 366 | the bridge arm, fully served |
| own-note ↔ paper | 167 | 47 | capped by `capture_limit=12` |
| own-note ↔ own-note | 149 | 0 | deliberately excluded, correctly |
| paper ↔ paper | 71 | **0** | **no arm exists** |
| code ↔ own-note | 68 | 12 | capped by `capture_limit=12` |
| coursework ↔ own-note | 66 | 2 | capped by `capture_limit=12` |
| code ↔ paper | 62 | **0** | **no arm exists** |
| code ↔ coursework | 48 | **0** | **no arm exists** |
| code ↔ code | 16 | 0 | plausibly worth excluding |
| career ↔ own-note | 13 | 1 | |

**His code repos are the single largest hole, and they are exactly the side he says
produces the best connections** ("a paper's method against my project's stored profile").
A repo is never a source (`_bridge_sources` filters `source_type != 'code'`) and never a
bridge target (target must be `category='coursework'`). Pool reach per repo:

```
Downside Risk Prediction     36    Digest                       26
Regime-Conditioned Equity ML 32    Tanker-Flow Signal Derivation 25
Alpha Fund                   21    Python Solutions              20
Optiver Trading Academy      16    OXDAQ Infra / Member Portal    0
```

### 1.3 Two cheap eligibility levers, measured

```
                                    eligible pairs
current (12 notes + bridge)                    431
+ capture arm walks all 46 owner-authored      685    (+59%)
+ code repos as a source                       796    (+85%)
+ paper<->paper arm                            867   (+101%)
```

Walking all 46 owner-authored documents instead of the 12 newest is a **one-constant
change that adds 254 eligible pairs with no new arm and no new gate**. The `capture_limit=12`
was tuned for "what he is thinking about now" and it is doing real damage: 120 of the 167
own-note↔paper pairs are lost to it.

### 1.4 A guard that guards nothing

`_substantive_shared` applies `non_topical_names()`. Measured over the 1,615 multi-word
canonicals it can see: **it rejects 0.** Its 230 multi-word entries are document titles,
and those are already excluded inside `_CANON_CTE`; everything else in the set is
single-token and already dead by the multi-word rule.

This is harmless today and dangerous the moment we widen supply — the multi-word rule is
the obvious thing to relax, and relaxing it silently promotes `non_topical_names` from
inert to load-bearing with nobody having measured it in that role. §3 failure shape.

### 1.5 A hypothesis I checked and discarded

I expected `_SAMPLE_NAMES = 5` (related.py returns only the 5 rarest shared names) to be
hiding qualifying concepts. **It is not**: over the live walk, 138 pairs had a qualifying
concept visible in the sample, **0** had one hidden by it. Not a lever.

---

## 2. Quality

### 2.1 What we actually know

The feedback loop is close to dark. `acceptance_log` holds **2** `connection` rows, both
`kept`, and `candidate_key` is **the target document's URI** — not the pair, not the
concept. So a cross-out of the `while loop` bridge would have penalised the whole
*Introduction to Computer Engineering* document, and there is no pair-level or
concept-level label anywhere in the DB. There is no rejection recorded for `while loop`
at all.

**We cannot fit a threshold on this. Anything below is a hypothesis, not a measurement.**

### 2.2 Every discriminator for the 12 written notes

`df` = concept doc-frequency · `ent/sec` = entity occurrences / distinct sections per side ·
`syn` = concept appears in a synthesis field · `nq` = other qualifying concepts the pair
also shares · `minlen` = chars the smaller side actually handed the model.

| shared | df | sec_h | sec_o | syn_h | syn_o | nq | minlen | classes |
| --- | ---: | ---: | ---: | :-: | :-: | ---: | ---: | --- |
| regime | 6 | 1 | 1 | T | F | 0 | 1400 | own↔paper |
| regime | 6 | 0 | 1 | T | F | 0 | 1400 | own↔paper |
| deep reinforcement learning | 5 | 1 | 5 | F | T | 2 | 668 | own↔paper |
| **math fidelity** | 2 | 1 | 1 | F | F | 0 | 1400 | own↔project |
| null hypothesis | 2 | 1 | 1 | F | F | 3 | 1377 | paper↔coursework |
| Poisson process | 2 | 1 | 1 | F | F | 4 | 635 | paper↔coursework |
| **while loop** | 2 | 1 | 1 | F | F | 0 | **288** | project↔coursework |
| trading environment | 2 | 1 | 1 | F | F | 4 | 1400 | own↔paper |
| Optimal execution | 2 | 1 | 1 | F | F | 1 | 1320 | own↔paper |
| US Treasury market | 2 | 2 | 2 | F | F | 2 | 787 | own↔paper |
| portfolio construction | 13 | 1 | 3 | T | T | 0 | 1400 | own↔paper |
| deep reinforcement learning | 5 | 1 | 5 | F | T | 1 | 890 | own↔paper |

Three things fall out:

1. **`sec` has almost no dynamic range.** 19 of 24 sides anchor the concept in exactly one
   section (one anchors none; the remaining four are 2, 3, 5, 5). "Is this concept central to the document" is *not* answerable from `entities`
   at current extraction density. Any gate built on it will be a gate on noise.
2. **`minlen` separates the junk cleanly.** `while loop` handed the model 288 and 319
   characters. The next-thinnest connection is 635. That is a 2.2× gap with nothing in it.
3. **`syn ∨ nq≥2` is worse than it first looks.** It rejects `while loop` and
   `math fidelity` — but it *also* rejects `Optimal execution` (syn F/F, nq=1), which is
   one of the better connections on the list. One true negative, one arguable, one clear
   false positive out of twelve. `minlen` does strictly better on this evidence: at a
   600-char floor it rejects `while loop` and nothing else.

**Both rules are fitted to a single owner-labelled negative, and one of them already
mis-fires on a connection I'd call good.** I am not proposing either as a threshold. They
are the *hypotheses under test* in §4, instrumented through `locus gates` from the first
run so we find out what they reject. Note this also means the `P_cent` column in §2.4 is
pessimistic — it is discarding pairs of the `Optimal execution` shape.

### 2.3 The concept-selection bug

`_substantive_shared` returns the **first qualifying name** from `shared_names`, which
related.py orders **rarest-first**. Rarest is not best. Measured example from the
paper↔paper pool:

```
Advanced Portfolio Management  <->  AlphaZeroBeta (deep RL, market-neutral portfolios)
  56 qualifying concepts shared; rarest-first picks: 'Balance Sheets'
```

Against `Newey–West HAC t-statistic` or `equity index futures`, `Balance Sheets` is the
worst available choice. In the 12 written notes the same rule chose `trading environment`
over `mean reversion` / `market making` / `factor loading`, and `Poisson process` over
`Bayes' theorem` / `central limit theorem`. Rarity is a good tiebreak among *equally
central* concepts and a bad primary key. This is cheap to change and directly testable.

### 2.4 Supply × quality together

Eligible pairs surviving each candidate predicate:

| arm | none | `syn∨nq≥2` | `minlen≥600` | `minlen≥900` | both | both + `df≤12` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| current (12 notes + bridge) | 431 | 109 | 90 | 45 | 45 | 31 |
| + all 46 owner-authored | 685 | 228 | 208 | 100 | 116 | 90 |
| + code repos as a source | 796 | 285 | 255 | 112 | 157 | 107 |
| + paper↔paper | 867 | 340 | 298 | 129 | 194 | 144 |

Widest arm × tightest gate = **144 pairs**, composed of: own-note↔paper 47,
paper↔paper 37, code↔own-note 23, coursework↔paper 15, code↔paper 14, other 8.
At 3 seats/day that is ~48 days of CONNECT, from a surface that is currently exhausted —
and it survives a gate strictly tighter than today's.

Real examples the widened arm + gate would newly offer:

```
Downside Risk Prediction (repo)   <-> AlphaZeroBeta          'feature engineering'  nq=5
Alpha Fund (repo)                 <-> Advanced Portfolio Mgmt 'Risk aversion'       nq=3
Alpha Fund (repo)                 <-> AlphaZeroBeta          'covariance estimation' nq=3
Optiver Trading Academy (repo)    <-> AlphaZeroBeta          'market making'        nq=2
Inspectable Neural Markov Models  <-> Downside Risk (repo)   'realized volatility'  nq=2
Real-Time Exchange Simulator      <-> AlphaZeroBeta          'Market Microstructure Theory'
Macro-aware forecasting           <-> AlphaZeroBeta          'Newey–West HAC t-statistic'
Nonlinear/Heavy-Tailed Predict.   <-> AlphaZeroBeta          "Engle's ARCH-LM test"
```

---

## 3. Depth

### 3.1 The prompt is already truncating

`connect._doc_text` gives each side `thesis + method + result` plus up to 3 section
summaries found by `summary LIKE '%concept%'`, capped at `_MAX_SIDE_CHARS = 1400`.

**15 of 24 sides in the written notes hit that 1400 cap.** Nearly two thirds of the time the
model is *already* being cut off. "Would more depth help" is not a hypothetical — material
is being discarded right now.

### 3.2 The section lookup uses the wrong join

The pair exists *because* an entity resolved to a shared canonical. The prompt then finds
sections by **substring match on the summary**, which is a different, weaker question.

```
sides where a section summary CONTAINS the concept string : 18 / 24
sides where a section ANCHORS the concept as an entity    : 23 / 24
```

For `while loop`, `Poisson process`, `deep RL` (his side) and `US Treasury market` (his
side), `LIKE` found nothing and the entity join found a section. Switching to the anchor
join is strictly better-grounded *and* recovers material — it is the join that created
the candidate in the first place.

### 3.3 Marked passages cannot broaden supply yet

Only **two** documents in the corpus carry any ink:

```
Can Large Language Models Execute Parent Orders?   105 marks, 11,236 chars
Advanced Portfolio Management                       39 marks,  6,034 chars
```

So "feed it marked passages" is not a supply lever today. It is, however, the strongest
*relevance* signal that exists for those two documents, and both are heavily represented
in the pool. Worth testing as a depth variant on those pairs specifically — not worth
building an arm around.

### 3.4 The honest answer to "deeper or just longer?"

Unknown, and not answerable by measurement — only by his judgement on real output. That is
experiment **E3**. What measurement *does* say is that the cheapest depth win requires no
extra tokens at all: fix the section join (§3.2) so the 1,400 characters we already spend
are the right 1,400.

---

## 4. Proposed experiments

Ordered. Each produces something you judge, not something I assert.

### E0 — Build the label set (do this first; everything else depends on it)

Nothing can be thresholded against 2 acceptance rows and one remembered "that was junk".

Sample **~60 pairs stratified across the feature space** — `minlen` low/high, `nq` 0/1/2+,
`syn` T/F, and every class in the table above including the currently-unreachable ones —
write prose for each with the *current* prompt, and print them as a numbered rating sheet.
You score each 1–5 on "told me something I hadn't thought of and could act on."

Output: a labelled CSV joined to the deterministic features. Only then does §2.2 become a
threshold instead of a hypothesis.

Cost: ~60 calls. Note that `write_note` goes through `agent/claude.py`, which CLAUDE.md
§13 documents as env-scrubbed to subscription OAuth — so this is wall-clock, not $5-cap.
**Worth confirming before relying on it**; I did not verify it independently.

Alongside, a free fix worth making regardless: `_route_connection` should log acceptance
keyed by the **pair and concept**, not by `anchor.target_key`. Every day it stays as-is is
a day of labels being written to the wrong row.

### E1 — Source arms (judge from candidates, before any prose is written)

For each arm in §2.4, print the candidate list with features and let you strike the ones
that are obviously wrong. This is free — no model calls — and it tells us whether
`paper↔paper` (37 pairs at the tight gate) is a real capability or a distraction, and
whether `code↔*` delivers what you expect from it.

My prior, stated so you can disagree: **all-46-notes and code-repos-as-source are clear
wins; paper↔paper is the one I am least sure about**, because neither side is his and the
"what would you do with this" question has no obvious addressee.

### E2 — Concept selection (A/B, same pairs)

Three policies over the same 20 pairs, prose written for each, presented blind:

- **A** rarest qualifying (current)
- **B** highest IDF among concepts attested in *both* documents' synthesis fields, falling
  back to rarest
- **C** hand the model the full qualifying shared list and require it to name which one it
  built the question on (verified after the call against the list — §2 grounded-or-silent)

C is the most interesting and the most likely to fail the grounding check; that failure
rate is itself the result.

### E3 — Depth (A/B, same pairs)

Four variants over the same 15 pairs, blind:

- **A** current
- **B** entity-anchored sections instead of `LIKE`, same 1400 cap
- **C** B + cap raised to ~3000, up to 6 sections
- **D** C + propositions naming the concept; and for the two marked documents, the marked
  passages

You rate 1–5. The question we are answering is specifically whether D > C > B > A or
whether it plateaus at B — i.e. whether depth buys insight or just words.

### E4 — Gate, instrumented (only after E0)

Fit the gate to E0's labels rather than to `while loop`. Whatever it turns out to be, it
ships behind `gates.record` from the first run, per §3 — with a named check that fires if
its reject rate hits 100% or its accept rate hits 100%, because both mean it is not a gate.

Also worth folding in: **`Optibook Python Reference` is a vendor manual filed
`category='project'`**, and it is one of only three non-code "projects" with any coursework
reach. It is the actual cause of the `while loop` item. Re-categorising it is a data fix,
not an algorithm change, and it is cheaper than any gate that would have caught it.

---

## 5. What I would not do

- **Relax the multi-word rule to widen supply.** It is the only *live* constraint in
  `_substantive_shared` (§1.4), and eligibility — not the gate — is what is starving the
  surface. Widen arms first; the gate can stay tight and still triple supply.
- **Rank on section counts** (§2.2 point 1). No dynamic range.
- **Build a marked-passage arm** (§3.3). Two documents.
- **Raise `top_n` in the walk.** 8× cost for a fraction of what removing `capture_limit=12`
  gives free.
