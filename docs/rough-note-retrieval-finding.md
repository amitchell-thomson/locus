# Rough-note retrieval: the cause was `select()`, not the summary pass

**Date:** 2026-07-30
**Supersedes:** the EXTRACTION-BLOCKED diagnosis recorded in `locus/eval/retrieval_eval.py`
(2026-07-29) and in CLAUDE.md §15.

## What was recorded

After the `rough_penalty` sweep (14 queries × 4 penalty values), three rejected note queries
were unchanged at *every* penalty value. That was read as proof of a §11.B extraction ceiling:

> These notes never enter the candidate pool at all, so no scoring change can rescue them.
> Both are dense idea-lists whose GENERATED SUMMARIES flatten into generic prose that embeds
> near nothing specific. The fix is a stronger summary/proposition pass for maturity=rough
> docs (§11.B), not a retrieval knob.

The inference was too strong. The sweep varied exactly one knob (`rough_penalty`) and
concluded that because that knob changed nothing, the cause must be upstream of ranking. It
never tested the **selection** stage, which sits between the reranker and the survivors and
is where these documents were actually being dropped.

## What is actually true

Measured against the live corpus (`scripts/analysis/rough_diag.py`):

| Query (target) | units in raw pool | best unit's rerank score | pool rank | survived? |
|---|---|---|---|---|
| mispricings in CEE rates (`em-rates-trading`) | **13** | −2.665 | 1 | no |
| price discovery / AI risk reporting (`em-ideas`) | **2** | **+0.192** | **1** | no |
| Markov regime models bridge (`robert-training`) | 3 | −3.642 | 15 | no |
| internship ↔ BH offer bridge (`swaps-b3f4d16b`) | **0** | — | — | no |

Three of the four targets *are* in the candidate pool — `em-rates-trading` richly so, with 13
units. The claim that they "never enter the candidate pool at all" is false for those three.

For `em-ideas` the target section was **the single best-scoring unit in the entire pool**
(+0.192, with the runner-up 4.7 points behind) and still did not survive.

## The mechanism

`select()` demoted a `section` (summary) candidate whenever *any* proposition/chunk of the
same section appeared **anywhere in the pool**, on the stated assumption that

> expansion re-attaches the summary to the child for free.

That assumption holds only if the child is actually **selected**. For a rough note it
routinely is not. A captured handwriting note is one section whose summary reranks well and
whose single raw chunk is verbatim transcription — `<!-- page 1 -->\n\n# Ideas\n\n- click
through ⟦menus⟧\n\n- risk in 3m[?] gaps[?]` — which the cross-encoder scores near the bottom.

For `em-ideas` (`scripts/analysis/rough_redundancy_test.py`):

```
target units in pool:
  rank=1    section     section_id=7970 score=+0.192
  rank=42   chunk       section_id=7970 score=-10.461

selected 8 units; target present: False
```

The pool's rank-1 unit was suppressed in favour of a child at rank 42 that no cut would ever
reach. The refill pass could not rescue it either, because refill is gated on `min_score`
(0.22) and the section scored +0.192 — just below. Two independent guards, each locally
reasonable, combined to delete the document from the results entirely.

This is a general defect, not a rough-note one. It hits rough notes hardest because they are
short: one section, one chunk, and that chunk is raw handwriting. For a polished paper the
chunks are prose, the child *is* selected, and the rule behaves as designed.

## The fix

Suppress a section only when a child of the same section **makes the cut**. Which children
make the cut is resolved by a probe pass (`_children_in_cut`) that drops every section
candidate, giving children the most favourable competition they could face. That makes the
result a sound over-approximation in the direction that matters: a child absent from the
probe cut cannot appear in the real one, so suppressing its parent would delete the document.

The slot economy the original rule was built for is preserved — the existing regression test
(`test_select_drops_section_summary_when_child_in_pool`, where the child genuinely wins a
slot) still passes unchanged.

## Result

`scripts/analysis/rough_acceptance.py`, before → after:

| Query | before | after |
|---|---|---|
| mispricings in CEE rates | MISS | **rank 1** |
| price discovery / AI risk reporting | MISS | **rank 2** |
| Markov regime models bridge | MISS | MISS |
| internship ↔ BH offer bridge | MISS | MISS |

Two of the four recovered, at zero API cost. Note the first of these is the label
`retrieval_eval.py` deliberately retained as known-failing — it is the visible metric for
this class of defect, and it now passes.

## The two that remain — and why a stronger summariser is not the fix

**`robert-training` (Markov bridge).** The source note is genuinely near-empty. Page 3 is a
pen test (`tilly is the best` repeated in different weights and nibs); the rest is a bare
topic list — `Hidden Markov Models (HMMs)` written three times, `Credit stress is a key
issue`, `Bond Pricing`. There is no content about *how* HMMs apply to credit stress, so no
summariser can produce a summary that would match the query without inventing it. The stored
summary ("informal notes containing fragmented observations about financial concepts") is
**faithful to a fragmentary source**. Separately, the direct query for this note
(`How are hidden Markov models used for credit stress and risk attribution?`) is an existing
label and passes. This is content thinness, not extraction loss.

**`swaps-b3f4d16b` (internship ↔ offer bridge).** Zero units in the pool, and the note is
the opposite of thin — a well-structured markdown page deriving covered interest parity. It
misses because the query's vocabulary ("my internship work", "the Brevan Howard offer") has
no lexical or semantic overlap with the note's content (FX swaps, near/far legs, forward
rates). Answering it requires the *link* layer to connect a note to a career document, not a
better summary of either. This candidate was never promoted to a label.

## Standing lesson

A sweep over one knob bounds that knob's effect. It does not identify the cause. "Unchanged
at every penalty value" ruled out the reranker's *scores*; it did not rule out the
selection rules downstream of them, which is where the documents were actually lost.
