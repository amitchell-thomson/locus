# locus: where it stands

**As of:** 2026-11-18 · **State:** live

## Now
- Mail, calendar, tasks, chats, repos, library, syllabi and his notes in one index (heuris),
  served to every Claude (laptop, claude.ai, phone); facts ledger, records and briefs on top.
  Home, Approve, Tasks, Projects, Chat, System in the locus window; kairos and keryx in it.
- Canvas pull (spec P) done 15 Nov: files, deadlines, syllabi as topics linked to lectures
  (817 of 1,742 topics covered); Q31 passes.
- One locus MCP (18 Nov): kairos's and keryx's tools served through locus as `kairos_*` and
  `keryx_*`; Claude registers only `locus`. Mail tools need the laptop awake (keryxd reads Mac Mail).
- Learning (spec L) L1 to L3: interview-prep agreed 17 Nov, 32 topic tasks at 4 h a week in term
  and 8 in vacations, 45 to 90 minute sessions, done around 21 April 2027.
- 16 Nov bug hunt: six reviewers, ~70 findings; the serious ones fixed with tests (privacy
  scrubbing, mail envelopes, ledger duplicates, Approve acting on the wrong item, kairos and
  keryx approval bypasses, Claude's own MCP-only tokens).

## Decided
- 2026-11-18: one locus MCP for everything; Out of Bits is `out-of-bits`, grown from the G-Research AI/ML track.
- 2026-11-16: Explore / the nightly reading digest joins the first pass (ADR-012, fact:422).
- 2026-11-16: backups keep the newest 10; Oxford list mail stays reviewed in Approve for now.
- 2026-11-14: syllabi are first-class structure, the guide for L (spec P).
- 2026-11-13: order C, P, L; the Project window goes.

## Next
- The OXDAQ project plan with him, then the cpp-oxdaq curriculum (L8 brought forward).
- L4 sheets and L5 marking (sessions run live until then); progress in `docs/redesign/slices/l-learning.md`.
- Then the nightly reading digest: spec first.
- Then a closing pass over the bug-hunt leftovers and gap list (in this session's report).

## Open
- The public passphrase now guards mail too: rotate it if it is short or reused.
- Rotate the old master tokens for kairos and keryx (they sat in Claude's config).
- Oxford staff mail via lists counts as automated: change the rule, or keep catching it in Approve.
- CLAUDE.md has drifted (Canvas "next", `library_candidates`, units list): refresh it.
