# ADR-009 — A task says what it is for; how it is planned follows from its estimate

**Status:** accepted
**Date:** 2026-10-25
**Your words:** "I still have to enter a type of sheet coursework or admin and there are more things than that - what if I am scheduling laundry, or need to do interview prep, or whatever, those feel like a restrictive set and do not feel like they match the taxonomy we worked so hard to pin down"; "Type ahead is useful, I like that" (25 Oct 2026, Slice 1 round 7)
**Builds on:** ADR-006, ADR-008

## Context
ADR-006 gave every item an area and a kind, and gave a task one type: sheet, coursework, admin,
chore. kairos then asked for the type when adding a task, and for a calendar (a key) when adding
anything. Both questions are about labels you do not think in. What you know when you add
something is what it is for (B15, Millennium, the laundry) and roughly how long it takes. The
four types were really four planning shapes (block sizes), presented as categories. Interview
prep, a lab write-up and a parcel all had to be squeezed into one of them.

## Decision
Adding anything to the calendar asks **what it is for**: a registry item (a course, an
application, a repo) or one of Personal's streams (Family, Sport, Social, Errands), chosen by
type-ahead. That is the item's link to locus, set when it is made, and its calendar follows from
the item's area. A dentist appointment is Errands; a birthday Social or Family.

**How a task is planned follows from its estimate**: an hour or less in one sitting, longer in
blocks of two to three hours. A course's sheet keeps its five-hour default. The task types stop
being something you choose. They remain internal planning shapes the scheduler derives, and
"chore" means a short Errands task. This supersedes the task-type row of ADR-006's kinds table,
not its areas or kinds.

## Alternatives
- **Keep the types and add more** (prep, write-up, errand): a longer list of the same mistake,
  categories that are really block sizes.
- **Ask for the area instead of a type**: closer, but loses the link to the item (B15, not just
  Studies), which is what locus needs to show the task where it belongs.
- **A quick-add line** (`laundry 20m errands`): considered and declined; "happy filling in the
  fields, it is quick."

## Consequences
- Every task and event made in kairos carries its locus item from the start, so linking afterwards
  becomes the exception (`L` stays as the fallback).
- The scheduler's block sizes come from the estimate and the item's kind, not from a field.
- Tasks already stored keep their type; it becomes a planning hint, read when present.

## What would make this wrong
- If the derived shape plans badly often enough that you keep overriding it, the shape belongs
  back in the form, as a choice of planning shape ("in one go" or "spread"), not of category.

## Amendment, 30 Oct 2026: an area on its own is an answer
**Your words:** "can we add a generic career/ projects/ studies labels in the for field for things
that are general within that like planning general prep for the process, or projects that do not
have a thread yet and are in their infancy, and studies not related to a specific course that are
instead just general" (30 Oct 2026)

For also offers each area that has a calendar as `Studies (general)`, `Projects (general)`,
`Careers (general)`: the area's calendar and no registry item, the way a stream is. An item with
no link in an area calendar reads as that area's general one, with `L` to link it to something
specific. A project that grows a registry entry is linked with `L` then. The earlier reason for
not offering areas ("an event always has a calendar more specific than that") was wrong for work
that belongs to an area and to nothing in it yet.
