# Owner edits vs. the additive merge — the authority mechanism

**Date:** 2026-07-30
**Resolves:** the design fork flagged before Phase 3 step 3 (annotated-page pull-back).

## The conflict

`state.merge_body` is deliberately additive:

```python
# lists union preserving order; scalars fill ONLY where the stored value is empty
elif old in (None, "", [], {}):
    merged[key] = new
```

That asymmetry is load-bearing. It is what stops an agent re-proposal from deleting a thread
the owner is tracking — rule 2 of the propose-never-mutate invariant, alongside
`upsert_object` never writing `status`.

A hand-written correction on the daily page is the opposite case. If the owner crosses out
the agent's one-line "why" and writes their own, that text **must** replace the agent's. The
owner is the authority; the agent is not. But `merge_body` cannot express this — a non-empty
stored scalar is never overwritten, by anyone.

Weakening `merge_body` to allow overwrites would destroy the very property it exists for: the
next `locus structure` run would be free to clobber the owner's tracked threads.

## The mechanism: separate write paths, plus a durable authority marker

Two changes, neither of which relaxes the agent path.

### 1. Owner writes are a different verb

`upsert_object` stays exactly as it is — the **agent** path, additive, never writes `status`.
Owner edits go through a new `apply_owner_edit()`, which replaces scalars outright and can
remove list items. The invariant was never "no writer may overwrite"; it was "**the agent** may
not overwrite". Making the owner a distinct verb states that directly instead of relying on
emptiness as a proxy for authority.

This alone is not sufficient, because of resurrection.

### 2. Owner edits leave a marker the additive merge honours

Without a marker, an owner edit survives only until the next structure run: the owner deletes
a bullet, the agent re-proposes it, and `merge_body`'s list-union silently re-adds it. The
owner's correction would have to be re-made every night — which is exactly the "chore" the §9
longevity guardrails forbid.

So `apply_owner_edit()` records what the owner touched, in two reserved body keys:

```jsonc
{
  "why": "owner's replacement text",
  "threads": ["one the owner kept"],

  "_owner_edits":   {"why": {"at": "2026-07-30T...", "source": "daily:2026-07-30#obj-42"}},
  "_owner_removed": {"threads": ["the bullet the owner struck out"]}
}
```

`merge_body` then gains one rule, and it only ever makes the agent do **less**:

- a scalar named in `_owner_edits` is never written by the agent — *even when the stored value
  is empty*, because the owner may have deliberately cleared it;
- a list item recorded in `_owner_removed[field]` is never re-appended;
- the reserved keys are themselves not mergeable from an incoming agent body.

Everything else behaves as before.

## Why this keeps the invariant rather than eroding it

| | before | after |
|---|---|---|
| agent may add to a field | yes | yes |
| agent may overwrite a non-empty field | no | no |
| agent may fill a field the owner emptied | **yes** (bug) | no |
| agent may re-add an item the owner removed | **yes** (bug) | no |
| owner may overwrite | no (had to edit the DB) | yes, via `apply_owner_edit` |

The agent's write surface strictly shrinks. The two rows marked *(bug)* are cases where the
current additive merge silently overrides an owner decision — so this change closes two holes
in propose-never-mutate while opening the owner path the daily page needs.

## Provenance

`_owner_edits[field].source` carries the daily page and anchor the edit came from
(`daily:<date>#<anchor>`), so every owner-authored field traces to the physical page it was
written on — the same "provenance on everything" rule that governs agent writes, applied to
the owner's side. `apply_owner_edit` never writes `status`: blessing stays `set_status`, so
"the owner corrected this" and "the owner blessed this" remain separate, independently
auditable acts.

## What this does not cover

Body-level merge only. `belief_positions` are append-only by construction (a dated chain — a
correction is a *new* dated position, never an edit of an old one), and `review_schedule` is
owned by SM-2 alone. Neither needs an authority marker.
