Hand-written edge cases; expected behaviour is asserted in `crates/kairos-core/tests/semantics.rs`.

| file | covers |
|---|---|
| dst_weekly_sunday.ics | weekly Sun 19:00 Europe/London across 25 Oct 2026 and 28 Mar 2027 |
| dst_ambiguous.ics | 01:30 on 25 Oct 2026 (occurs twice → first) and 28 Mar 2027 (doesn't exist → pre-gap offset) |
| allday.ics | exclusive DTEND, multi-day span, missing DTEND |
| recur_count_until_exdate.ics | COUNT + EXDATE; UNTIL in UTC across DST |
| recur_override_moved.ics | RECURRENCE-ID moving an instance to another day |
| recur_cancelled.ics | cancelled instance |
| recur_thisandfuture.ics | RANGE=THISANDFUTURE and an iCloud-style split series |
| timezones.ics | floating, Europe/Vienna, UTC, Outlook "GMT Standard Time" |
| dedup_*.ics | must-merge (same UID, fuzzy title, Family copy) and must-not-merge (lectures, Squash on two days, moved vs original slot) |
| deadline_and_block.ics | all-day + timed (zero-length, alarms) deadlines, task block with X-KAIROS-* |
