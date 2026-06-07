"""Advisory ingest lock: one Ollama-driving process at a time (build step 7 rule).

Two concurrent ingests contend for the 8 GB GPU and produce spurious quarantines. This
was an honor-system operational rule; the flock turns it into a guardrail. Acquired at
the user-facing entry points (`locus ingest`, `locus sync`, each `locus watch` tick) —
NOT inside library functions, so the watch tick can hold one lock across its incoming
pass + repo sync without re-entrancy issues (flock from a second fd in the same process
would block).

Manual CLI commands fail fast with an actionable message; the watch loop catches
IngestLockHeld, logs, and skips the tick (it retries next interval anyway).
"""

from __future__ import annotations

import fcntl
from contextlib import contextmanager
from typing import Iterator

from locus.config import load

LOCK_NAME = ".ingest.lock"


class IngestLockHeld(RuntimeError):
    """Another ingest/sync process holds the lock."""


@contextmanager
def ingest_lock() -> Iterator[None]:
    """Hold the exclusive ingest lock for the duration of the block (non-blocking acquire).

    Raises IngestLockHeld immediately if another process holds it.
    """
    lock_path = load().paths.db.parent / LOCK_NAME
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    f = lock_path.open("w")
    try:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise IngestLockHeld(
                f"Another ingest/sync is running ({lock_path} is held); "
                "wait for it to finish or stop the watcher first."
            ) from None
        yield
    finally:
        f.close()  # releases the flock
