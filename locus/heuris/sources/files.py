"""Career file items from the pushed `files.json` (H1.12): one per file, read on the laptop."""

from __future__ import annotations

import re

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterator

from locus.heuris import registry
from locus.heuris.index import Item


def _thing(path: str) -> str | None:
    """A folder named for a firm with exactly ONE application -> that application, else None.

    A firm with two (Brevan Howard 2026 and 2027, both Millennium entries) is left to the text:
    `brevan-howard/` holds the 2026 internship's contract, and guessing the newer one filed it
    under the 2027 offer."""
    parts = {p.lower().replace("-", "").replace(" ", "") for p in Path(path).parts[:-1]}
    hits = [pid for pid, e in registry.entries().items()
            if (c := (e.get("company") or "").lower().replace("-", "").replace(" ", "")) and c in parts]
    return hits[0] if len(hits) == 1 else None


def _thing_by_text(text: str) -> str | None:
    """The application a file is about, from what it says, for a file whose folder names no firm
    (H1 fix 2, 10 Nov: the Millennium offer letter sits in ~/Downloads and was linked to nothing,
    so `timeline millennium` and four answers never saw it).

    The firm must be named in the text, and only one firm; between that firm's applications
    the rest of each application's name decides (the letter says "2027" and "Internship", the
    2026 event's name says "Meet"). A tie gives none: an unlinked file is findable by search,
    a wrongly linked one misleads."""
    low = text.lower()
    named = lambda w: re.search(rf"(?<![a-z0-9]){re.escape(w.lower())}(?![a-z0-9])", low) is not None
    apps = [(pid, e) for pid, e in registry.entries().items() if e.get("kind") == "application" and e.get("company")]
    firms = {e["company"] for _, e in apps if named(e["company"])}
    if len(firms) != 1:
        return None
    firm = firms.pop()
    scored = []
    for pid, e in apps:
        if e["company"] != firm:
            continue
        rest = [w for w in e.get("name", "").split() if w.lower() not in firm.lower() and len(w) >= 2]
        scored.append((sum(1 for w in rest if named(w)), pid))
    scored.sort(reverse=True)
    if not scored or (len(scored) > 1 and scored[0][0] == scored[1][0]):
        return None
    return scored[0][1]


def items(root: Path) -> Iterator[Item]:
    f = root / "files.json"
    try:
        rows = json.loads(f.read_text()).get("files", [])
    except (OSError, ValueError):
        return
    for r in rows:
        thing = _thing(r["path"]) or _thing_by_text(r.get("text") or "")
        yield Item(
            ref=f"file:{r['root']}/{r['path']}",
            kind="file",
            at=datetime.fromtimestamp(r["mtime"], tz=timezone.utc),
            title=f"{r['root']}/{r['path']}",
            text=r["text"],
            area="Careers" if r["root"] in ("career", "downloads") else registry.area_of(thing),
            thing=thing,
            meta={"root": r["root"], "path": r["path"], "sha": r.get("sha")},
        )
