"""Identify a staged reMarkable render: `<uuid>.pdf` -> (name, folder, category).

The device pushes each changed notebook to the server as `<uuid>.pdf` (the xochitl document id;
transport built in Phase 0), landing in a staging dir. Those files are opaque — to file a capture
as a named, categorised note, the uuid must be mapped to the document's human name and its device
folder. Confirmed server-side, no device SSH (agent-layer plan / Phase-0 findings):

    rmapi find /            -> every document's path
    rmapi stat <path>       -> {"ID": <uuid>, "Name": ..., "Parent": ...}

so `{uuid: (name, top_folder)}` is a `find` plus one `stat` per document (the corpus is small;
this is a cheap periodic lookup). The top folder maps to a Locus category via config.

The rmapi subprocess is behind an injectable `RmapiRunner`, so the mapping logic is unit-testable
without the cloud. Documents in excluded folders (trash, our own push target, admin) are skipped;
a staged uuid with no cloud match (device-only / not yet synced) is reported, not guessed.
"""

from __future__ import annotations

import json
import logging
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

log = logging.getLogger(__name__)

# argv AFTER the rmapi binary -> (returncode, stdout, stderr). Default shells out; tests fake it.
RmapiRunner = Callable[[list[str]], "tuple[int, str, str]"]

# reMarkable top-folder -> Locus CATEGORY. Category is a COARSE *kind* facet, not the organisation
# (2026-07-28 decision): the folder is capture context (a topic), not a content-kind, so it is
# preserved as note provenance + drives concept/entity linking — NOT mirrored into a taxonomy.
# So handwriting defaults to `note` (default_category); only folders that denote a genuine KIND
# override. Owner-tunable via [capture]; empty config falls back to this.
DEFAULT_FOLDER_CATEGORY: dict[str, str] = {
    "engineering": "coursework",  # handwritten lecture annotations = study material
    "quantum_ml": "coursework",
    "projects": "project",         # notes tied to a project
}
# Folders whose documents are NOT Loop-A handwriting capture: trash (deleted), Locus (our own
# pushed reading target, invariant 5), admin (born-digital personal paperwork, not notes).
DEFAULT_EXCLUDED_FOLDERS: tuple[str, ...] = ("trash", "Locus", "admin")


@dataclass(frozen=True)
class CaptureItem:
    uuid: str
    pdf_path: Path   # the staged <uuid>.pdf
    name: str        # human document name from reMarkable
    folder: str      # top-level device folder
    category: str    # mapped Locus category
    # `rmapi stat`'s ModifiedClient — WHEN THE OWNER LAST WROTE IN THE NOTEBOOK, not when capture
    # ran. It becomes the note's `date:` frontmatter and hence documents.source_date, and hence
    # belief_positions.dated_at. Without it every captured note is dated "today" and the whole
    # understanding-evolution trajectory (§6.3) collapses to a flat line at the capture date —
    # which is exactly what happened to the first 12-document capture. None if the device did not
    # report one.
    modified: str | None = None


def _subprocess_runner(binary: str) -> RmapiRunner:
    def run(args: list[str]) -> tuple[int, str, str]:
        proc = subprocess.run([binary, *args], capture_output=True, text=True, timeout=120)
        return proc.returncode, proc.stdout, proc.stderr

    return run


def _find_file_paths(runner: RmapiRunner) -> list[str]:
    """Device paths of all documents (`[f]` entries) from `rmapi find /`."""
    code, out, err = runner(["find", "/"])
    if code != 0:
        raise RuntimeError(f"rmapi find failed: {err.strip() or out.strip()}")
    paths = []
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("[f] "):
            paths.append(line[4:].strip())
    return paths


def _stat(runner: RmapiRunner, device_path: str) -> dict | None:
    """`rmapi stat <path>` parsed to a dict, or None if it failed/was unparseable."""
    code, out, err = runner(["stat", device_path])
    if code != 0:
        log.warning("rmapi stat %r failed: %s", device_path, err.strip() or out.strip())
        return None
    try:
        return json.loads(out)
    except json.JSONDecodeError:
        log.warning("rmapi stat %r returned non-JSON: %r", device_path, out[:200])
        return None


def build_uuid_index(
    runner: RmapiRunner, *, excluded_folders: tuple[str, ...] = DEFAULT_EXCLUDED_FOLDERS
) -> dict[str, tuple[str, str, str | None]]:
    """Map every non-excluded document's `uuid -> (name, top_folder, modified)` via find + stat.

    `modified` is `ModifiedClient` — the device's own last-edited timestamp, which is the only
    honest date for handwriting (see CaptureItem.modified)."""
    index: dict[str, tuple[str, str, str | None]] = {}
    for device_path in _find_file_paths(runner):
        folder = device_path.lstrip("/").split("/", 1)[0]
        if folder in excluded_folders:
            continue
        meta = _stat(runner, device_path)
        if not meta or not meta.get("ID"):
            continue
        index[meta["ID"]] = (
            meta.get("Name") or Path(device_path).name, folder, meta.get("ModifiedClient"),
        )
    return index


def identify_staged(
    staging_dir: str | Path,
    *,
    runner: RmapiRunner | None = None,
    rmapi_binary: str = "rmapi",
    folder_category: dict[str, str] | None = None,
    excluded_folders: tuple[str, ...] = DEFAULT_EXCLUDED_FOLDERS,
    default_category: str = "note",
) -> tuple[list[CaptureItem], list[str]]:
    """Resolve every `<uuid>.pdf` in `staging_dir` to a `CaptureItem`.

    Returns `(items, unmapped_uuids)`: `unmapped_uuids` are staged files with no cloud match
    (in an excluded folder, trashed, or not yet synced to the cloud) — reported so a real gap is
    visible, never silently ingested with a guessed category."""
    staging = Path(staging_dir)
    runner = runner or _subprocess_runner(rmapi_binary)
    cats = {**DEFAULT_FOLDER_CATEGORY, **(folder_category or {})}
    index = build_uuid_index(runner, excluded_folders=excluded_folders)

    items: list[CaptureItem] = []
    unmapped: list[str] = []
    for pdf in sorted(staging.glob("*.pdf")):
        uuid = pdf.stem
        if uuid not in index:
            unmapped.append(uuid)
            continue
        name, folder, modified = index[uuid]
        items.append(
            CaptureItem(
                uuid=uuid, pdf_path=pdf, name=name, folder=folder,
                category=cats.get(folder, default_category), modified=modified,
            )
        )
    return items, unmapped
