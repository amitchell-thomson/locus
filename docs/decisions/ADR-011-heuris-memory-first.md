# ADR-011 — heuris is the system's memory: one index, current facts, tools any Claude can call; the slices reorder around it

**Status:** accepted 10 Nov 2026 (heuris.tex v2: his tablet markup of v1 folded in; "agree with all four, lets go")
**Date:** 2026-10-28
**Your words:** "I sort of see heuris as a context tool about me that anything anywhere can call and get context about something" and "I want it to be a core runner in the system that supports and works intelligently with everything"; "indexing is a good idea, that sounds like a fundamental requirement"; "having heuris.db and locus.db sounds messy, can we migrate everything into heuris.db"; "I would put the bulk work on the api, and anything else if we are sure it will stay under the limit"; "change to invariant 1 correct"; "I just want to build the best system possible however we can, and if this order gets us there then that is good with me" (28 Oct 2026)
**Builds on:** ADR-006, ADR-007

## Context
The plan (v4) built heuris as dossiers, answers, the scout and a learning system, fed by a frozen
library and by change records, with "search over everything" last (Slice 7). Measured on 28 Oct:
since 8 Sep the MCP served 86 calls, 77 of them `markups` and `to_remarkable`; corpus search ran
once. Knowledge questions went to Claude inside the project repository, where locus could not be
reached. The failures Alec named were mostly not about documents: a chat thought he was still
waiting on Jump after he had said otherwise elsewhere; a project was misremembered for his CV;
nothing knew what he had studied. And some were about documents: links between projects and
notes or course material were not found. Mail (7,271 messages), chats (241 laptop sessions),
calendar and repositories were in no index at all. Separately, Max 5x turns out to include $100 a
month of API credit that expires unused.

## Decision
1. **heuris is a service, not a set of screens.** One database (`heuris.db`, absorbing `locus.db`)
   holds readings of sources, one index of items over every source (mail except OQTS-shared, the
   calendar including Family, chats, repositories, sources, ink), records, a ledger of current
   facts, briefs, and learning state. Mail and calendar data stay in keryx and kairos; heuris
   indexes them.
2. **Tools any Claude can call**, from Claude Code on both machines, claude.ai (through Tailscale
   Funnel) and the ask line: `context`, `search` (with kind, area, thing, person and date
   filters), `open`, `timeline`, `remember`, `forget`. Acting stays with the owning app: kairos
   schedules; keryx gains list-needs-reply, read and draft tools, and never sends.
3. **Facts from chats need approval** (`remember` stays off the allow list); facts from mail and
   calendar are written automatically with their source, logged with undo.
4. **Answers cite like references** and explain around their sources; facts and decisions are
   recalled exactly. Invariant 1 holds for everything stored.
5. **Locus's model calls run on the API credit** (Batch API and caching wherever nobody waits),
   paced by a ledger; the subscription is the fallback. This reverses part of Slice 0.13.
6. **The notes repository becomes `files`**; transcriptions are items, not files.
7. **Slices reorder** (10 Nov, to his 9 Nov priorities): 4 Home live (from the registry, git,
   kairos and tmux; needs nothing from heuris) → 0 finishing (the 0.6 service, the credit ledger)
   → H1 the index and the tools (the Home chat gets search) → H2 what is true now (facts from
   mail are "mail that derives truth": application status, deadlines, cleanup) → L learning and
   revision. Set aside with the 9 Nov scope cut, back only on request: 3 Project view, 2 dossiers,
   5 linking/Explore/scout, 7 Canvas.
8. **The first test is the question set** Alec wrote or approved (heuris.tex Part J), with planning
   and learning questions added 10 Nov.
9. **Added from the v1 markup (10 Nov):** the Home chat approves `remember` with its own `y/n`
   line (a headless turn cannot show a permission prompt; the refused call carries the fact); a
   PDF he wrote on replaces the clean copy in `~/files` (no `.ink.pdf` sibling; the original stays
   in the raw store by hash); PDFs added on the tablet come down into `~/files`; briefs are
   rebuilt when a fact changes and `context` returns any newer records with them; one learning
   track serves several targets, and a new topic starts with a baseline quiz. `~/LocusDrop` and
   `~/LocusVault` were removed.

## Alternatives
- **Keep the plan's order** (dossiers first, search in Slice 7): builds the most expensive outputs
  on top of an engine nobody could reach, and leaves the named failures unfixed until last.
- **Fixed retrieval then one call** (the old `query`): cannot follow a question across mail, chats
  and facts, and cannot tell a newer fact from an older one.
- **Extract facts from chats automatically:** hypotheticals discussed in a chat would become
  facts; "that would kill the system".
- **Two databases** (library and heuris): two places to look, migrate and back up for one
  system.
- **Stay entirely on the subscription:** spends the weekly limit he works with, leaves the
  included credit to expire, and leaves unattended `claude -p` jobs in a grey area of the terms.

## Consequences
- Search over everything, area briefs and the "context for every chat" idea (plan C.13) arrive
  first rather than last.
- The old engine's derived layers (propositions, entities, aliases, links, threads, connect,
  structure, evolve, critique, synthesise, decide, the Obsidian export) are dropped, propositions
  only after a retrieval evaluation shows answers do not get worse without them.
- The Home budget bar shows credit and pace; the System view gains a heuris section.
- `agent/claude.py` gets a second backend; CLAUDE.md §0, §2.1 and §13 need updating when this is
  accepted.
- Exposing the service through Tailscale Funnel needs an authentication claude.ai accepts, not
  yet verified.

## What would make this wrong
- If, after H1, the test questions still fail for lack of facts rather than lack of finding, H2
  should have come first.
- If Alec approves nearly every `remember`, the approval step is friction and facts from chats
  can be written automatically.
- If the credit ledger shows the month routinely running short, the briefs or the ask line move
  back to the subscription.
- If chats do not call `context()` unprompted, the global instruction is not enough and
  something stronger (a session-start hook) is needed.
