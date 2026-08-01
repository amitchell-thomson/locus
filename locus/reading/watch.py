"""Observe the accept signal: where each proposed reading now sits on the device.

The gesture is the whole design. Moving a file out of `Locus/Reading/Proposed` costs the owner
nothing he was not already doing, and it is unambiguous — nobody drags a paper into `In-Progress`
by accident. Compare the alternative of a tick box on the daily page, which asks him to judge
something he has not read yet.

WHY THIS DOES NOT REUSE `capture/remarkable.build_uuid_index()`, which looks like the right tool:

  1. it EXCLUDES the `Locus` folder by default, and deliberately — that exclusion is what stops
     Loop A ingesting our own pushed pages as though they were his handwriting (invariant 5);
  2. it keeps only the TOP-level folder (`path.split("/", 1)[0]`), so every reading would read as
     `Locus` whether it sits in `Proposed`, `In-Progress` or `Finished`. The one field the accept
     signal is made of is the field it discards;
  3. it costs one `rmapi stat` per document in the whole account.

A single `rmapi find /Locus/Reading` answers the question directly, because we control the
filename we uploaded. That is one call per pull instead of N.

MATCHING IS BY NAME, NOT UUID. `rmapi find` returns paths, not ids, and the device drops the
`.pdf` extension from a document's display name — so a delivered `2026-07-31 Foo.pdf` appears as
`.../Proposed/2026-07-31 Foo`. Comparison is therefore on the STEM. `device_uuid` is still
recorded at delivery and used as a secondary key, because a rename would break name matching and
we would rather notice that than silently reject his reading.

SAFETY RULE, and it is not paranoia: **an empty or failed listing changes nothing.** A transient
rmapi failure that returned no rows would otherwise look exactly like "he deleted all of them",
and would mass-reject his entire reading list in one pass.
"""

from __future__ import annotations

import logging
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

from locus.reading import proposals as P
from locus.reading.deliver_remarkable import RmapiRunner, _subprocess_runner

READING_FOLDERS = P.READING_FOLDERS

log = logging.getLogger(__name__)

DEFAULT_ROOT = "/Locus/Reading"
# How long a proposal may sit untouched in `Proposed` before it is read as a no. Deliberately
# generous: this is the WEAK negative (see `proposals.channel_stats`), and three weeks of silence
# is as likely to mean a busy fortnight as a bad suggestion.
DEFAULT_TTL_DAYS = 21


@dataclass(frozen=True)
class DeviceEntry:
    # ABSOLUTE device path, reconstructed rather than taken from rmapi's output. `rmapi find`
    # renders paths relative to the parent of whatever it searched (`Reading/In-Progress/X`), and
    # feeding that straight to `rmapi get` fails — which is exactly how the first real annotation
    # sweep failed, on a paper the owner had genuinely moved and marked up.
    path: str
    folder: str   # reading folder it currently sits in ('Proposed', 'In-Progress', 'Finished')
    stem: str     # document display name, without the .pdf the device strips


@dataclass
class Outcome:
    proposal_id: int
    title: str
    action: str            # 'accepted' | 'rejected' | 'held'
    resolution: str | None  # 'moved' | 'ttl' | 'removed' | None
    folder: str | None


def _stem(name: str) -> str:
    """Device display name for a delivered file: the basename with any .pdf removed."""
    return Path(name).stem if name.lower().endswith(".pdf") else name


def list_reading_entries(
    runner: RmapiRunner, *, root: str = DEFAULT_ROOT
) -> list[DeviceEntry]:
    """Every document under the reading root, with the subfolder it currently sits in.

    Raises on an rmapi failure rather than returning an empty list — see the module's safety rule.
    """
    code, out, err = runner(["find", root])
    if code != 0:
        raise RuntimeError(f"rmapi find {root!r} failed: {err.strip() or out.strip()}")

    # THE FOLDER IS THE FILE'S IMMEDIATE PARENT — do not try to match the root as a prefix.
    #
    # rmapi renders paths relative to the PARENT of whatever it was asked to search, and without a
    # leading slash: `rmapi find /Locus/Reading` returns `Reading/Proposed/<name>`, while
    # `rmapi find /Locus` returns `Locus/Reading/Proposed/<name>`. Prefix-matching the root
    # therefore matched nothing at all, and every delivered paper read as "no longer under the
    # reading root" — which the scan would have scored as him deleting it. Ten papers were live on
    # the device while the watch saw zero.
    #
    # It survived unit testing because the fixture was written with the format the code expected
    # rather than the one rmapi emits; only running it against the device exposed it. Taking the
    # parent directory is invariant to how rmapi chooses to render the prefix, so this cannot break
    # again if the rendering changes.
    entries: list[DeviceEntry] = []
    for line in out.splitlines():
        line = line.strip()
        if not line.startswith("[f] "):
            continue  # '[d] ' entries are the folders themselves
        path = line[4:].strip()
        parts = [p for p in path.split("/") if p]
        if len(parts) < 2:
            continue  # a file with no parent folder belongs to no reading folder
        folder = parts[-2]
        if folder not in READING_FOLDERS:
            continue  # something else living under the root — not part of the accept signal
        stem = _stem(parts[-1])
        entries.append(DeviceEntry(
            path=f"/{root.strip('/')}/{folder}/{stem}", folder=folder, stem=stem,
        ))
    return entries


def _parse(ts: str | None) -> datetime | None:
    if not ts:
        return None
    try:
        parsed = datetime.fromisoformat(ts)
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def scan(
    conn: sqlite3.Connection,
    *,
    runner: RmapiRunner | None = None,
    rmapi_binary: str = "rmapi",
    root: str = DEFAULT_ROOT,
    ttl_days: int = DEFAULT_TTL_DAYS,
    now: datetime | None = None,
) -> list[Outcome]:
    """Resolve every `proposed` reading against where it now sits on the device.

    Returns one `Outcome` per proposal that was still awaiting a verdict. Accepted proposals are
    left at `status='accepted'` — turning one into a corpus document is `ingest_accepted`'s job,
    deliberately separate so that a transport read can never itself write to the corpus.
    """
    runner = runner or _subprocess_runner(rmapi_binary)
    now = now or datetime.now(timezone.utc)

    pending = P.list_proposals(conn, status="proposed", limit=500)
    if not pending:
        return []

    entries = list_reading_entries(runner, root=root)
    if not entries:
        # Everything gone at once is far likelier to be a bad listing than a mass deletion.
        log.warning("reading watch: %r listed no documents — leaving %d proposal(s) untouched",
                    root, len(pending))
        return []

    by_stem = {e.stem: e for e in entries}
    cutoff = now - timedelta(days=ttl_days)
    outcomes: list[Outcome] = []

    for prop in pending:
        entry = by_stem.get(_stem(prop.filename or ""))
        if entry is None:
            # Not under the reading root any more: deleted, or moved somewhere we do not watch.
            # A deliberate removal is the STRONG negative, distinct from letting it sit.
            P.set_status(conn, prop.id, "rejected", resolution="removed")
            P.record_verdict(conn, prop.dedupe_key, "rejected")
            outcomes.append(Outcome(prop.id, prop.title, "rejected", "removed", None))
            continue

        if entry.folder != P.FOLDER_PROPOSED:
            P.set_status(conn, prop.id, "accepted", device_folder=entry.folder,
                         resolution="moved")
            P.record_verdict(conn, prop.dedupe_key, "kept")
            outcomes.append(Outcome(prop.id, prop.title, "accepted", "moved", entry.folder))
            continue

        proposed_at = _parse(prop.proposed_at)
        if proposed_at and proposed_at < cutoff:
            P.set_status(conn, prop.id, "rejected", resolution="ttl")
            P.record_verdict(conn, prop.dedupe_key, "rejected")
            outcomes.append(Outcome(prop.id, prop.title, "rejected", "ttl", entry.folder))
            continue

        outcomes.append(Outcome(prop.id, prop.title, "held", None, entry.folder))

    return outcomes
