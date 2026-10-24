# JSON output (schema 1)

Every read command takes `--json`. The schema is versioned with a top-level `"schema": 1`; fields are only ever **added** within a version. Times are RFC 3339 with the local offset (Europe/London); all-day dates are `YYYY-MM-DD` with an **exclusive** end.

## `kairos ls|week|today --json`

```json
{
  "schema": 1,
  "timezone": "Europe/London",
  "from": "2026-10-15T00:00:00+01:00",
  "to": "2026-10-22T00:00:00+01:00",
  "events": [
    {
      "uid": "…",                     // iCalendar UID
      "recurrence_id": "20261020T090000Z" | "20261023" | null,
      "title": "B4 Lecture",
      "start": "2026-10-20T10:00:00+01:00" | "2026-10-23",
      "end":   "2026-10-20T11:00:00+01:00" | "2026-10-24",
      "all_day": false,
      "date": "2026-10-20",            // local start date
      "overlay": "personal" | "family" | "background" | "imported",
      "key": "Engineering" | null,     // colour key (Family events keep their origin key)
      "source": "engineering",         // config source id
      "location": "LR1" | null,
      "kind": "event" | "deadline" | "task_block",
      "flex": false,
      "task_id": "t-1a2b3c4d" | null,
      "created_by": "alec" | "claude" | "scheduler" | null,   // null = not created via kairos
      "recurring": true,
      "tentative": false,
      "transparent": false,
      "colour": "#89b4fa",
      "duplicates": [ { "uid": "…", "source": "oqts-shared", "overlay": "personal" } ]
    }
  ]
}
```

Duplicates are hidden copies (view-only dedup, §5); the displayed one is chosen by precedence Family > iCloud personal > read-only import > Imported.

## `kairos next --json`

```json
{ "schema": 1, "kind": "event" | "deadline" | "none", "text": "󰃰 Squash 19:00 (in 2h)",
  "uid": "…", "title": "Squash", "start": "2026-10-15T19:00:00+01:00", "minutes_until": 120 }
```

## `kairos task ls --json`, `kairos changes --json`, `kairos plan --json`

These return the daemon's objects directly: `Task`, `Changeset` and `PlanResp` from `crates/kairos-core/src/{task,changeset,api}.rs` (same field names as the HTTP API and MCP tools).
