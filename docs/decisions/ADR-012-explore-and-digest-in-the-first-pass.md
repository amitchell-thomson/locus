# ADR-012 — Explore and the nightly reading digest join the first pass

**Status:** accepted
**Date:** 2026-11-16
**Your words:** "I think we should add explore and nightly reading digest to the list of things to implement in this first locus pass, getting locus to a clean, refined useable state - so record that decision before we start on learning." (16 Nov 2026)
**Builds on:** ADR-011

## Context
On 13 Nov the order was set: the cleanup (C), then the Canvas pull (P), then Learning (L). Explore
had been set aside on 9 Nov with the scope cut, and its redesign as a nightly reading digest was
kept as a todo he liked but did not schedule: a ~30-minute curated read before sleep that probes
new ideas or teaches something, suggests extensions of projects, fixes to shortfalls and life
plans; he writes notes on it, and the ideas it raised and the ones he noted are saved and
re-proposed later. C and P are done (P closed 15 Nov), and L is next.

## Decision
Explore, redesigned as the nightly reading digest, is part of the first pass: the work that
takes locus to a clean, refined, usable state. It is no longer an open todo outside that pass.
Like every slice it gets its own spec, approved before anything is built (ADR-002). L stays next;
the digest follows it unless he orders otherwise.

## Alternatives
- Leave it as a later todo (13 Nov): rejected by him; the first pass would end without the part
  of locus that generates ideas rather than only recalling them.

## Consequences
- The first pass is now: C (done), P (done), L, then Explore / the digest, then a closing pass
  over what remains of the inventory (16 Nov bug hunt and gap list).
- The digest builds on what exists: heuris search over the library, facts and briefs for what he
  is working on, `to_remarkable` for paper, and capture's marks for his notes on it. Its spec
  must say how a raised idea and his note on it are stored and re-proposed.

## What would make this wrong
If L shows that the syllabus and topic structure is what the digest should draw from first, the
digest's spec should wait for L's data rather than duplicate it.
