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


def _stages() -> dict[str, str]:
    """Each application's stage from his applications file (pushed beside the repos)."""
    import tomllib

    from locus.heuris.run import inbox

    try:
        data = tomllib.loads((inbox() / "repos" / "_applications.toml").read_text())
    except (OSError, ValueError):
        return {}
    return {k: str(v.get("stage", "")) for k, v in data.items() if isinstance(v, dict)}


def _thing_by_text(text: str, stages: dict[str, str] | None = None) -> str | None:
    """The application a file is about, from what it says, for a file whose folder names no firm
    (H1 fix 2, 10 Nov: the Millennium offer letter sits in ~/Downloads and was linked to nothing).

    The firm must be named in the text, and only one firm. Between that firm's applications:
    a document about an OFFER goes to the one whose stage is an offer or a contract (the first
    version of this read "meet" and the letter's 2026 date and linked the offer letter, pack and
    handbook to Meet Millennium 2026: worse than no link); then the rest of each application's
    name, matched as written (capitalised), so ordinary words like "meet" do not count. A tie
    gives none: an unlinked file is findable by search, a wrongly linked one misleads.
    """
    stages = _stages() if stages is None else stages
    # The opening only: a letter or a form says whom it is from at the top, and a book that
    # mentions Millennium on page 200 is not about the internship (Advanced Portfolio Management
    # was linked to it, 10 Nov).
    text = text[:3000]
    low = text.lower()
    named = lambda w: re.search(rf"(?<![a-z0-9]){re.escape(w.lower())}(?![a-z0-9])", low) is not None
    exact = lambda w: re.search(rf"(?<![A-Za-z0-9]){re.escape(w)}(?![A-Za-z0-9])", text) is not None
    apps = [(pid, e) for pid, e in registry.entries().items() if e.get("kind") == "application" and e.get("company")]
    firms = {e["company"] for _, e in apps if named(e["company"])}
    if len(firms) != 1:
        return None
    firm = firms.pop()
    about_offer = named("offer") or named("contract") or named("employment")
    scored = []
    for pid, e in apps:
        if e["company"] != firm:
            continue
        score = 0.0
        if about_offer and re.search(r"offer|contract|signed", stages.get(pid, ""), re.I):
            score += 2
        rest = [w for w in e.get("name", "").split() if w.lower() not in firm.lower() and len(w) >= 2]
        score += sum(1 for w in rest if not w.isdigit() and exact(w))
        scored.append((score, pid))
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
