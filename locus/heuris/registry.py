"""The registry (taxonomy/registry.toml) as heuris needs it: id -> area, kind, folder, sessions."""

from __future__ import annotations

import tomllib
from functools import lru_cache
from pathlib import Path

PATH = Path(__file__).resolve().parents[2] / "taxonomy" / "registry.toml"


@lru_cache(maxsize=1)
def entries(path: Path = PATH) -> dict[str, dict]:
    data = tomllib.loads(path.read_text())
    return {p["id"]: p for p in data.get("projects", [])}


def area_of(thing: str | None) -> str | None:
    return entries().get(thing, {}).get("area") if thing else None


def by_folder(folder: str) -> str | None:
    """The registry id whose folder (or session name) is `folder`, repos first."""
    hits = [(p.get("kind") != "repo", i) for i, p in entries().items()
            if p.get("folder") == folder or folder in p.get("sessions", [])]
    return min(hits)[1] if hits else None
