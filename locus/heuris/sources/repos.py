"""Repository items from the pushed project files (H1.7): one per doc, one per commit."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterator

from locus.heuris import registry
from locus.heuris.index import Item


def items(root: Path) -> Iterator[Item]:
    for f in sorted(root.glob("*.json")):
        try:
            d = json.loads(f.read_text())
        except (OSError, ValueError):
            continue
        pid = d["id"]
        area = registry.area_of(pid)
        name = registry.entries().get(pid, {}).get("name", pid)
        for doc in d.get("docs", []):
            yield Item(
                ref=f"doc:{pid}/{doc['path']}",
                kind="doc",
                at=datetime.fromtimestamp(doc["mtime"], tz=timezone.utc),
                title=f"{name}: {doc['path']}",
                text=doc["text"],
                area=area,
                thing=pid,
                parent=f"repo:{pid}",
                meta={"path": doc["path"], "folder": d.get("folder")},
            )
        for c in d.get("commits", []):
            repo = c.get("repo") or ""
            where = f"{pid}/{repo}" if repo else pid
            files = c.get("files", [])
            text = c["subject"] + (f"\n\n{c['body']}" if c.get("body") else "")
            if files:
                text += "\n\nFiles: " + ", ".join(files[:40])
            yield Item(
                ref=f"commit:{where}@{c['sha']}",
                kind="commit",
                at=datetime.fromisoformat(c["at"]),
                title=f"{name}{' ' + repo if repo else ''}: {c['subject']}",
                text=text,
                area=area,
                thing=pid,
                parent=f"repo:{pid}",
                people=[(c.get("author", ""), "author")],
                meta={"sha": c["sha"], "repo": repo, "files": files},
            )
