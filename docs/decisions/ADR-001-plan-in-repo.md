# ADR-001 — The plan lives in the repository

**Status:** accepted
**Date:** 2026-10-19
**Your words:** "yes go ahead" (19 Oct 2026), to: put the plan into the repository, orient
CLAUDE.md to the redesign, and write the Slice 0 spec.

## Context
The plan was written and marked up on paper over four versions (14–19 Oct 2026). Its LaTeX source
lived in a session scratchpad, which was deleted mid-way; v2 had to be recovered from the session
transcript. Sessions that build locus need the plan without a human pasting it in.

The existing CLAUDE.md is 706 lines about the current engine, much of it hard-won invariants
(silent-failure class, VRAM choreography, idempotent ingest) that stay true for the library.

## Decision
- `docs/redesign/plan.tex` (v4, final) and `info.tex` are the source of truth for the redesign.
- CLAUDE.md gets a new section 0 on top: what is changing, where the plan is, how the build runs,
  and which existing sections describe parts being removed. The existing sections are kept intact
  and shrink as their code is removed slice by slice, rather than being rewritten up front.

## Consequences
- Any session sees the redesign first and the engine detail second.
- Two documents for a while (plan and CLAUDE.md engine sections) — the section-0 table says which
  engine sections are on their way out.

## What would make this wrong
If sessions keep acting on engine sections marked for removal, or if the plan and CLAUDE.md drift
apart; then CLAUDE.md is rewritten in full.
