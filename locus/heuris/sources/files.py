"""Career file items from the pushed `files.json` (H1.12): one per file, read on the laptop."""

from __future__ import annotations

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


def items(root: Path) -> Iterator[Item]:
    f = root / "files.json"
    try:
        rows = json.loads(f.read_text()).get("files", [])
    except (OSError, ValueError):
        return
    for r in rows:
        thing = _thing(r["path"])
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
