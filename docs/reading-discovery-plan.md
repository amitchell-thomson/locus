# Reading discovery — proposed reading list, delivered to the tablet

**Status:** NOT BUILT. Design note only.
**Referenced from:** `locus/learn/reread.py`, `locus/agent/compose_daily.py` (`build_readings`).
**Date:** 2026-07-30

## What exists today, and why it is only half the idea

`learn/reread.py` ranks documents **already in the corpus** by how many open gaps each would
close, and the daily page's read-next slot shows the top few. That is deterministic, free, and
honest — but it can only ever say *"re-read this"*.

The owner's framing, verbatim:

> It should not just be a gap filler but also a way to find relevant other research/ideas and
> promote discovery of new methods/etc.

That is a different product. Gap-filling closes holes in what you already have; **discovery
finds things you did not know to look for.** The second is where the compounding value is, and
it is the one thing in the system that requires reaching outside the corpus.

## The loop

```
corpus signal  ->  candidate works (outside the corpus)  ->  Locus/Reading/Proposed  (PDF on tablet)
                                                                      |
                                     owner moves it on the device     |
                                                                      v
                                         Locus/Reading/In-Progress  or  Locus/Reading/Finished
                                                                      |
                                          folder move observed  ->  INGEST as a real document
```

**The folder move is the accept signal, and it is a good one.** It costs the owner nothing extra
— he is already moving things around on the tablet — and it is unambiguous: nobody moves a paper
to In-Progress by accident. Compare the alternative of a tick box on the daily page, which asks
him to make a decision about something he has not read yet.

`Proposed` is a holding pen with no obligation attached; things left there are a rejection
signal, and after N days they can be dropped silently (never with a count, §9).

## What drives the proposals

Two sources, and the second is the interesting one:

1. **Gap-driven** — a concept his blessed work uses that no corpus document explains well.
   This is `reread.open_gap_concepts()` minus the concepts that already have good coverage.
2. **Adjacency-driven (discovery)** — the corpus's own shape suggests where to look next:
   - authors/venues that recur across his highest-value documents;
   - concepts that co-occur with his concepts in the papers he already has, but which he has no
     document for at all (the entity graph's frontier);
   - the reference lists of papers he has engaged with most — cheapest and highest-signal, since
     a citation from a paper he valued is already a filtered recommendation;
   - methods adjacent to ones he uses (a paper implementing regime detection cites three other
     regime methods he has never read).

(2) is what makes it discovery rather than a to-do list. It should be able to surface a method
he has never named, because the material he values points at it.

## Where the candidates come from

Out of scope for the corpus itself, so this needs one external source. Options in rough order of
effort:

- **Reference extraction from owned PDFs** — no network, no API, works today. Parse the
  bibliography of high-engagement papers, resolve titles, rank by how often a work is cited
  across his corpus. Probably the right first implementation.
- **arXiv API** — free, no key, category + keyword queries, good metadata. Natural second step
  for "new work in the areas he cares about".
- **Semantic Scholar / OpenAlex** — citation graph and recommendations proper; needs a key
  (S2) or is heavily rate-limited (OpenAlex), but gives real "papers like this" edges.

Fetching the actual PDF is only legal/possible for open-access work; for anything else the
proposal is a **stub page** carrying title, authors, abstract, why-it-was-proposed and a link —
which is enough for the owner to decide, and the ingest happens when he supplies the real file.

## Invariants this must honour

- **Grounded-or-silent.** Every proposal carries *why*, tied to a real gap or a real citation
  from a real document. "You might like this" with no provenance is exactly the noise that
  teaches him to stop looking at the folder.
- **Propose, never mutate.** A proposed reading is not a corpus document. It becomes one only
  when he moves the file, and then it goes through the ordinary ingest spine with no special
  casing.
- **`_generated/`-style separation.** Proposal stubs are agent output; they must not be ingested
  as if they were his material, and must not feed back into the signal that produced them.
- **Hard caps, no guilt.** A handful of proposals at a time. Never a count of what is unread.
- **The flywheel applies.** Moved → `kept`; still in Proposed after N days → `rejected`. That is
  a real relevance label about *discovery quality*, and it should feed candidate ranking the way
  `acceptance_log` already feeds related-document ranking.

## Transport

Already solved in both directions — reuse it, do not invent a second path:

- **out:** `reading/deliver_remarkable.deliver_pdf()` (`rmapi put`), same as the daily page.
  Note the daily page's lesson: **date or otherwise uniquify the filename**, because rmapi
  refuses a same-named re-upload rather than duplicating.
- **back:** the device already pushes every changed document over the tailnet into the staging
  dir (`scripts/remarkable/receiver.py`), and `capture/remarkable.build_uuid_index()` reports
  each document's **folder**. So the folder move is observable with code that already exists —
  compare a document's current folder against its last seen folder.

That last point is the reason this design is cheap: the accept signal needs no new plumbing at
all, only a record of which folder each proposal was last seen in.

## Smallest first version

1. `Locus/Reading/{Proposed,In-Progress,Finished}` on the device.
2. Reference extraction over the top ~20 highest-engagement corpus PDFs; rank by cross-corpus
   citation count, drop anything already in the corpus.
3. Emit ≤3 stub PDFs a week into `Proposed`, each with its grounded why.
4. A folder-watch pass (hourly, alongside `locus daily-pull`) that notices a move out of
   `Proposed` and ingests the real file when one is present.
5. Log the outcome to `acceptance_log` under a `reading` surface.

Steps 1, 4 and 5 are small. Step 2 is the real work, and it is worth doing before touching the
arXiv/S2 network sources — a citation from a paper he actually read is a stronger signal than
anything a keyword query returns.
