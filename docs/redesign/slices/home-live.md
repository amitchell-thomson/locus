# Home live

**State:** proposed 10 Nov 2026 · **Written:** 2026-11-10 · **Runs in:** `claude` (locus-dev)

**From:** your 9 Nov priorities ("wished home page project view was live"), first in the build
order of heuris v2 (accepted 10 Nov). It needs nothing from heuris.

**Done when:** every row on Home shows something true about today, and you stop checking tmux,
git and your inbox by hand to know where things stand. Nothing on Home comes from a fixture.

---

## What is live today, and what is not (measured 10 Nov)

| Section | Source now |
| --- | --- |
| Top line: next event, mail counts | **live** (kairosd, keryx) |
| Work week grid | **live** (kairos logs) |
| Approve, tasks, chores | **live** (kairosd, keryxd) |
| Studies: what is due | **live** (kairos tasks) |
| The chat bar | **live** |
| Personal projects: sessions, what each is doing, last activity, "new" counts | fixture |
| Careers: each application's stage and whose move | fixture |
| Explore | fixture (Explore is set aside) |
| Learning goals row under studies | fixture (L comes later) |
| System: running jobs, done/failed today, budget | fixture |

The tmux check also found drift the fixture hid: `aulos`, `gr-ai-ml`, `med-ml-3yp` and
`b1-numerical-analysis` are live sessions that match nothing in the registry, and 3YP's registered
session `B3-Project` no longer exists.

## Decisions for you (my recommendation first)

- **H1, sessions read from tmux** `[proposal]`. Every tmux session is matched to a registry
  entry by its `sessions` list, or else by the folder its panes sit in. A pane running Claude
  Code is:
  - **working** when its screen shows "esc to interrupt";
  - **needs you** when it shows a permission question;
  - otherwise **idle**, since the transcript was last written.

  What it is doing is the session's last prompt from you, cut to one line, from its transcript.
  Read every 5 seconds; costs nothing.
- **H2, a live session the registry does not know is shown, not hidden** `[proposal]`. It appears
  in projects as `aulos · not registered`, and Enter asks the chat to register it. The registry
  stays the one list; nothing is guessed into it.
- **H3, last activity from git** `[proposal]`. A project with no session shows its last commit:
  `last 8 Nov  barrier sweep on the 5y grid`. A course shows what is due, as now.
- **H4, the "new" counts go** `[proposal]`. They needed change records (heuris H2). A count that
  is always zero teaches you to ignore it, so the column is removed until then.
- **H5, careers from a file you and the chat keep, until heuris H2 writes it** `[proposal]`.
  - `~/files/Careers/applications.toml` holds one entry per application: `stage`, `turn`
    (`you`/`them`/none) and `since`.
  - The chat may already edit `~/files`, so "Jump said no" in the chat updates it.
  - I seed it from the fixture and you correct it once.
  - Heuris H2 later writes the same file from mail, so Home does not change again.
  - The alternative is to wait for H2, leaving careers fake until then.
- **H6, Explore leaves Home; the learning row waits for L** `[proposal]`. Both were fixture-only
  and are set aside. Their space goes to projects and careers.
- **H7, the system section goes live from the server** `[proposal]`. It shows:
  - jobs running now (`systemctl --user` on the server);
  - done and failed today (the run journal `locus status` reads);
  - one budget line from `locus budget`.

  Fetched over ssh every 5 minutes on a thread of its own; when the server cannot be reached the
  section says so instead of showing old numbers.

## Steps

- **S1, sessions** (H1, H2): a `sessions` module in locus-tui reads `tmux list-panes`, captures
  the last lines of each Claude pane, and reads the newest transcript for the prompt. It lays
  the result over `projects[*].view.sessions`, as `calendar::apply` does for the calendar.
  Tests cover the state rules on captured pane text from real panes.
- **S2, last activity** (H3): `git log -1` for each active repo, read every minute.
- **S3, careers file** (H5): the contract, the seeded file, the chat brief told about it.
- **S4, system** (H7): one `locus status --json` on the server, with budget included (a flag
  added to the existing command). Home reads it over ssh.
- **S5, Home trimmed** (H4, H6): the count column removed, Explore and the goals row taken out;
  the fixture files for them stay on disk (set aside, not deleted).
- **S6, check against real output:** Home dumped with `--live` next to `tmux ls`, `git log` and
  the server's `locus status`, each row compared by hand; then a day of your use.

## Not in this

- Anything from heuris: briefs, facts, "where you are", summaries written by a model.
- The Project view, Explore, the scout, learning.
- Registering the four unregistered sessions: that is the chat's job, with you (H2).

## Report

(written when the steps are done)
