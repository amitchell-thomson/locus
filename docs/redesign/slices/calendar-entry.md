# Adding to the calendar: what it is for, moving, linking, dates

**State:** done 2026-10-25 · **Written:** 2026-10-25 · **Runs in:** `locus slice 1`
**Decision:** [ADR-009](../../decisions/ADR-009-for-not-type.md) (proposed). **From:** Slice 1's
round 7 (slice-1.md). **Builds on:** [kairos-joins-locus.md](kairos-joins-locus.md) (done).

**Done when:** adding anything in kairos asks what it is for and not a calendar or a type; a task
plans itself from its estimate; the selected item moves by 30 minutes or a day from the keyboard;
an interview's prep moves with it; interview dates found in your mail reach the calendar through
Approve, linked; and projects and tasks each have one overview, in the locus window.

---

## Decisions only you can make

**Answered 25 Oct 2026:** all as recommended ("all as recommended, approved, go ahead and do all
of them"). On E-b: "I especially like the tasks view idea on home, and would want to nail down how
that gets shown on home, so we stay along the design standard of any important view being shown
somehow on home."

| # | Question | Options | Recommendation |
|---|---|---|---|
| E-a | Where projects are overviewed | a. Home's personal projects section is the overview; kairos's projects view goes, and `P` in kairos opens that section in locus · b. keep a projects view in kairos, redesigned | **a.** Home already shows each project with its sessions and time; the measured time kairos keeps feeds it. One place, not two. |
| E-b | Where tasks are overviewed | a. a Tasks page in the locus window, from Home (and `T` in kairos opens it): grouped by what each is for (B15, Millennium, Errands), each with its due date, time left, next block and how often it was put off; done, more time and drop work there, through kairosd · b. keep kairos's task list and lay it out properly | **a.** Tasks belong to their items, which is what locus knows; the calendar keeps acting on blocks and flags. |
| E-c | Shift-←/→ | a. it nudges the selected item a day, and weeks move with `[` `]` · b. it nudges only in a "move" mode (`m` then arrows), and Shift-←/→ keep moving weeks | **a.** Something is nearly always selected in the week, so a mode would cost a key on every nudge. |
| E-d | "Setting tasks like prep for sheets" | a. a course can carry a weekly sheet (every week, due Fri 17:00, about 5 h), and the next one is made each week; an interview can carry prep (planned before it, moving with it) · b. something else you meant | Tell me which you meant; **a.** as a first reading. |
| E-e | Where interview dates from mail are found | a. a locus job reads keryx's local mirror (read-only, as the earlier research did) and puts a dated item in Approve · b. keryx finds them itself, once it joins this repository (ADR-008: "keryx will later too") | **a.** now, moving into keryx when it joins. |

## Agreed in round 7 (not open)
For replaces Calendar and Type, by type-ahead over the registry and Personal's streams; planning
from the estimate; no quick-add line; Shift-↑/↓ moves 30 minutes, Shift-←/→ a day; `L` links
afterwards; dates from mail; a dentist appointment is Errands, a birthday Social or Family;
moving an interview moves its prep; a due date is a flag at its time, or at the top with the
all-day items when it has none.

## Rules
- kairosd stays the only writer; everything here is a kairos changeset with an undo.
- A date from mail is never put on the calendar without you: it is an Approve item until you
  accept it.
- Mail-derived text in fixtures follows the usual rule: no addresses, links, ids or third parties'
  names.

---

## Steps

### E1 For, when adding and afterwards
- kairos's add-event and add-task forms lose Calendar and Type and gain **For**: type-ahead over
  the registry's items and Personal's streams (recent ones first), from the taxonomy kairos
  already reads. The calendar follows from the item's area; the item is the link to locus.
- `L` on any event or task sets For afterwards, with the same type-ahead.
- **Check:** a test per form that For sets the key and the link; a snapshot of the type-ahead;
  `L` on an unlinked event links it, and Enter then opens it in locus.

### E2 Planning from the estimate
- The scheduler derives a task's planning shape: an hour or less in one sitting, longer in blocks
  of two to three hours; a course's sheet five hours by default. Stored types are read as hints.
- **Check:** the scheduler's tests on the derived shapes; a 20-minute Errands task planned in one
  short block in a small gap; a 4-hour B15 task in two or three blocks.

### E3 Nudging
- Shift-↑/↓ moves the selected event or block 30 minutes; Shift-←/→ a day (per E-c); each nudge
  is one changeset with an undo, a block nudged by you becomes pinned.
- **Check:** key tests; the TUI's frame before and after a nudge.

### E4 Prep follows its interview
- A task can be planned before an event (prep before the interview): its due date is the
  event's start and follows it when the event moves, and the plan reflows.
- **Check:** moving the interview moves the prep's due date and its blocks (engine test).

### E5 Due dates on the day
- A due date with a time is a flag at that time; with no time, at the top with the all-day items.
- **Check:** snapshots of both.

### E6 Dates from mail
- Per E-e: bookings and rebookings for registry applications found in your mail become Approve
  items in locus ("Millennium · interview Tue 09:30–10:15 → calendar"); accepting one creates
  the event through kairosd, linked to the application; a rebooking offers to move the existing
  event. The Application page's `c` becomes a small "add a date" form.
- **Check:** on your real mail, the bookings found for one application listed for you to judge;
  one accepted end to end and seen on its timeline.

### E7 One overview each
- Per E-a and E-b: the projects overview is Home's section, opened from kairos with `P`; the
  tasks overview is a locus page, opened from Home and from kairos with `T`.
- **Check:** dumps of the Tasks page at 95 and 145 columns on your real tasks; `P` and `T` from
  kairos landing in locus.

## Order
E1 and E2 first (they change what adding asks), then E3 to E5, then E7, then E6 (it needs the
mail side). Each step is a commit of its own.

## Report

Run 25 Oct 2026, every step, as approved. 366 tests, clippy clean; deployed, and the locus
session's windows restarted on the new builds.

| Step | Commit | Evidence |
|---|---|---|
| E1 For | 0ad5d6a | Form tests (type-ahead order, completion); TUI test (For offered from the registry, matches drawn as you type, L opens the link form); `kairos task add --for`. |
| E2 planning | 0ad5d6a | Engine test: B15 sheet 4 → 5 h in blocks; laundry → 20 min chore; Millennium prep 3 h → blocks; 40 min → one go; a 3YP task joins its project; a long task with no estimate asks for one. |
| E3 nudging | 3332940 | Test of where a nudge lands (30 min, a day, the time kept); `[` `]` weeks; help and docs. |
| E4 prep | c36a8b3 | Engine test: prep bound to its linked interview; the interview moved; the prep's due date follows. |
| E5 due dates | 99fa1b0 | A real bug found by the check: a due date with no time showed on no day at all (it fell at the next day's midnight). Fixed; test with both kinds on one day. |
| E7 overviews | 4e49a35, 8120885 | Home at 95x38 keeps every row with the tasks line; overlay and page tests; live dump of your tasks (one open, eight done this week). |
| E6 mail | f141d86 | `--scan-mail 120` on your mail: Jump 16 Sep 15:00, every Millennium round with its length (the final 16 Oct 16:00, 30 min), Jump's visits, Citadel's assessment; quoted headers, "electronically" and a society's sponsor list no longer misread; Approve overlay tests. |

### What went differently
- **The tasks line sits in "waiting for you"** (its second row, its own key `d`), not as a
  section of its own: at 95x38 a separate section pushed a project and two applications off
  Home. Explore gives up its second find at the smallest window.
- **The Application page's `c` still adds only the next date.** Any other date is added in kairos
  with For set to the application, which now does the linking; a second form in locus would
  duplicate it.
- **Family is not a For.** It is the shared calendar; a family event goes in through `s` (share),
  as before. A birthday is Social, or shared to Family.
- **kairos's own task and project lists stay** as the fallback when the locus window is not
  running.
- **Dates from mail are read without a model**, so a date offered is a date the mail states; a
  firm with two live applications and no year in the subject offers nothing rather than guess.

