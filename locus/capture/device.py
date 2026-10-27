"""The tablet as the reMarkable cloud sees it, and what each document on it is (Slice 0.12).

WHY THIS EXISTS (2026-10-23). Capture used to wait for an agent ON THE TABLET to render each
changed document and push it over the tailnet to `locus-remarkable-receiver`. That agent stopped
on or about 1 Aug (a tablet update is the likely cause) and nothing said so: the receiver kept
listening, every capture run reported "0 captured, ~30 unmapped" and exited 0, and twelve weeks
of handwriting never reached the library. Loop B's change detection keyed on the same renders,
so it went quiet with it. Reading the CLOUD instead needs nothing installed on the device and
survives tablet updates, which is why reading ink already worked this way.

ONE CALL, NOT ONE PER DOCUMENT. The old index ran `rmapi find /` and then `rmapi stat` on every
document it did not exclude, about 80 cloud calls every 30 minutes, which fits the HTTP 429s in
the capture logs. rmapi keeps the whole tree in its own cache (`~/.cache/rmapi/tree.cache`) and
brings it up to date on every invocation, so one cheap `rmapi ls /` followed by reading that
file gives every document's folder, name, kind, modification time and per-file content hashes.
Measured 2026-10-27: 264 entries; the cached `.pdf` hash is sha256 of the PDF bytes, the same
key as `documents.content_hash` (6 of the device's PDFs matched library rows exactly).

The cache is rmapi's private format, which makes this a coupling to a pinned-by-accident tool
(CLAUDE.md §13). So the parse is strict about the few fields it uses and RAISES when they are
missing: a capture run that reads an unrecognised cache must fail visibly in the journal, never
report an empty tablet.

WHAT A DOCUMENT IS decides how it is read (Alec, 23 Oct: "pdfs are source, notebooks are my
writing, and any pdf claude sends should have my notes AND what my notes where about ingested"):

  notebook (no PDF in the bundle)  -> his writing: rendered and transcribed by Loop A
  a PDF Locus sent                 -> never ingested itself; his notes on it are promoted WITH
                                      the passage each was written against (Loop B)
  any other PDF in an area folder  -> a source for that area, ingested; his ink read as marks
  anything in Reading/             -> Loop B's reading rules, unchanged

Only the area folders, `Inbox`, `Reading` and notebooks at the root are read at all. Everything
else (`Admin`, `Archive`, `trash`, a folder he makes tomorrow) is ignored by construction rather
than by an exclusion list, so a new folder can never silently opt into billed transcription.

FOLDER LEVELS. Level 1 is the area, matched case-insensitively. Level 2 is the course, repo or
application, matched to `taxonomy/registry.toml` by name (in Careers, by the application's
`company`, since a firm's folder spans its applications). Anything deeper is his own
organisation and decides nothing. A level-2 folder the registry does not know keeps its area and
is reported, so a typo or an unregistered course is named rather than filed under nothing. A firm
with several applications matches all of them, and then no single one is recorded: the folder
says which firm, not which application.
"""

from __future__ import annotations

import hashlib
import json
import logging
import tomllib
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

log = logging.getLogger(__name__)

# Level-1 folder (casefolded) -> the area it is. The areas are the taxonomy's (ADR-006);
# Personal has no tablet folder: personal writing stays in Jot (plan C.8).
AREA_FOLDERS: dict[str, str] = {"studies": "Studies", "projects": "Projects", "careers": "Careers"}
# The library's category for each area, the same mapping the drop folder uses (ADR-003).
AREA_CATEGORY: dict[str, str] = {"Studies": "coursework", "Projects": "project", "Careers": "career"}
INBOX = "Inbox"
READING = "Reading"

DEFAULT_TREE_CACHE = Path("~/.cache/rmapi/tree.cache")
REGISTRY_PATH = Path(__file__).resolve().parents[2] / "taxonomy" / "registry.toml"


@dataclass(frozen=True)
class DeviceDoc:
    uuid: str
    name: str
    folders: tuple[str, ...]      # the folders above it, root first ("Studies", "B14 ...")
    is_notebook: bool             # no PDF in the bundle: written on the tablet from scratch
    pdf_hash: str | None          # sha256 of the PDF bytes, the library's content_hash
    ink_key: str                  # changes when the ink or the PDF changes, and only then
    ink_pages: int                # pages carrying a stroke layer
    modified: str | None          # ISO time of the device's lastModified

    @property
    def path(self) -> str:
        return "/" + "/".join((*self.folders, self.name))

    @property
    def top(self) -> str:
        return self.folders[0] if self.folders else ""


@dataclass(frozen=True)
class Placement:
    role: str                     # 'notebook' | 'sent' | 'source' | 'reading'
    area: str | None = None       # 'Studies' | 'Projects' | 'Careers' | None
    thing: str | None = None      # registry id of the course, repo or application
    thing_folder: str | None = None  # the level-2 folder name, matched or not
    matched: bool = False         # the registry knows the folder (it may name several things)

    @property
    def category(self) -> str:
        """The library category a document placed here is filed under."""
        return AREA_CATEGORY.get(self.area or "", "note")


def _iso_from_ms(value) -> str | None:
    try:
        ms = int(value)
    except (TypeError, ValueError):
        return None
    if ms <= 0:
        return None
    return datetime.fromtimestamp(ms / 1000, tz=timezone.utc).isoformat()


def _ink_key(files: list[dict]) -> str:
    """A fingerprint of the stroke layers and the PDF, and nothing else.

    The document-level hash in the cache also moves when `.metadata` does, which happens when he
    MOVES or opens a document; keying on it would re-transcribe a notebook (billed) because it
    was filed into a new folder. Ink and the PDF are what capture reads, so they are the key.
    """
    h = hashlib.sha256()
    for f in sorted(files, key=lambda f: f.get("DocumentID", "")):
        doc_id = f.get("DocumentID", "")
        if doc_id.endswith(".rm") or doc_id.endswith(".pdf"):
            h.update(f"{doc_id}={f.get('Hash', '')};".encode())
    return h.hexdigest()


def parse_tree(data: dict) -> list[DeviceDoc]:
    """Every live document in an rmapi tree cache, with the folders above it.

    Raises ValueError when the cache does not have the shape this reads: see the module
    docstring for why an unreadable cache must never look like an empty tablet.
    """
    docs = data.get("Docs")
    if not isinstance(docs, list):
        raise ValueError("rmapi tree cache has no 'Docs' list: its format has changed")
    by_id: dict[str, dict] = {}
    for d in docs:
        if not isinstance(d, dict) or "DocumentID" not in d or "Metadata" not in d:
            raise ValueError("rmapi tree cache entry without DocumentID/Metadata")
        by_id[d["DocumentID"]] = d

    def folders_of(meta: dict) -> tuple[str, ...] | None:
        """Folder names from the root down; None when it is trashed, deleted or orphaned."""
        names: list[str] = []
        parent = meta.get("parent") or ""
        seen: set[str] = set()
        while parent:
            if parent == "trash" or parent in seen:
                return None
            seen.add(parent)
            node = by_id.get(parent)
            if node is None:
                return None
            pmeta = node.get("Metadata") or {}
            if pmeta.get("deleted"):
                return None
            names.append(pmeta.get("visibleName") or "")
            parent = pmeta.get("parent") or ""
        return tuple(reversed(names))

    out: list[DeviceDoc] = []
    for doc_id, d in by_id.items():
        meta = d["Metadata"] or {}
        if meta.get("type") != "DocumentType" or meta.get("deleted"):
            continue
        folders = folders_of(meta)
        if folders is None:
            continue
        files = d.get("Files") or []
        pdf_hash = next(
            (f.get("Hash") for f in files if str(f.get("DocumentID", "")).endswith(".pdf")),
            None,
        )
        out.append(DeviceDoc(
            uuid=doc_id,
            name=meta.get("visibleName") or doc_id,
            folders=folders,
            is_notebook=pdf_hash is None,
            pdf_hash=pdf_hash,
            ink_key=_ink_key(files),
            ink_pages=sum(str(f.get("DocumentID", "")).endswith(".rm") for f in files),
            modified=_iso_from_ms(meta.get("lastModified")),
        ))
    return out


def read_device(runner, *, tree_cache: Path | str = DEFAULT_TREE_CACHE) -> list[DeviceDoc]:
    """Bring rmapi's tree cache up to date with one cloud call, then read it.

    `rmapi ls /` syncs the cached tree before it lists anything, so after a successful call the
    file is current. A failed call raises: a 429 or an auth failure leaves the cache as it was,
    and reading a stale tree as if it were today's would hide the failure (§14).
    """
    code, out, err = runner(["ls", "/"])
    if code != 0:
        raise RuntimeError(f"rmapi ls / failed: {(err or out).strip()[:300]}")
    path = Path(tree_cache).expanduser()
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as exc:
        raise RuntimeError(f"cannot read the rmapi tree cache {path}: {exc}") from exc
    return parse_tree(data)


# --- the registry: level-2 folders -> the course, repo or application they are -------------------

@dataclass(frozen=True)
class Registry:
    # (area, casefolded folder name) -> the registry ids it names, in registry order
    by_name: dict[tuple[str, str], tuple[str, ...]]

    def match(self, area: str, folder: str | None) -> tuple[str, ...]:
        if not folder:
            return ()
        return self.by_name.get((area, folder.casefold()), ())


def load_registry(path: Path | str = REGISTRY_PATH) -> Registry:
    """Read `taxonomy/registry.toml`. An application matches on its `company` first, then name.

    Missing or unreadable, the registry is EMPTY rather than an error: every document still gets
    its area, and every level-2 folder is reported as unmatched, which is loud enough.
    """
    try:
        data = tomllib.loads(Path(path).read_text())
    except (OSError, tomllib.TOMLDecodeError) as exc:
        log.warning("registry unreadable (%s): level-2 folders will all report unmatched", exc)
        return Registry({})
    by_name: dict[tuple[str, str], tuple[str, ...]] = {}
    for entry in data.get("projects", []):
        area, ident = entry.get("area"), entry.get("id")
        if not area or not ident:
            continue
        # An application is matched by its company: `Careers/Brevan Howard` is the firm's folder
        # and names BOTH of its internship applications. Which one a note belongs to is not in
        # the folder, so `place` records no single thing for it rather than the first one
        # registered (which filed 2026 internship notes under the 2027 application, 27 Oct).
        for key in {k.casefold() for k in (entry.get("company"), entry.get("name")) if k}:
            ids = by_name.get((area, key), ())
            if ident not in ids:
                by_name[(area, key)] = (*ids, ident)
    return Registry(by_name)


def place(doc: DeviceDoc, *, sent_hashes: set[str], registry: Registry) -> Placement | None:
    """How a document is read, or None when capture leaves it alone (see the module docstring)."""
    if not doc.folders:
        # The root holds Jot. A notebook here is his; a loose PDF has no home and is left alone.
        return Placement("notebook") if doc.is_notebook else None
    top = doc.top
    if top == READING:
        return Placement("reading")
    if top == INBOX:
        if doc.is_notebook:
            return Placement("notebook")
        # The Inbox is where Locus sends things. A PDF that Locus did not send has no rule here,
        # so it is left alone rather than guessed at.
        return Placement("sent") if doc.pdf_hash in sent_hashes else None
    area = AREA_FOLDERS.get(top.casefold())
    if area is None:
        return None
    thing_folder = doc.folders[1] if len(doc.folders) > 1 else None
    ids = registry.match(area, thing_folder)
    thing = ids[0] if len(ids) == 1 else None
    if doc.is_notebook:
        role = "notebook"
    elif doc.pdf_hash in sent_hashes:
        role = "sent"
    else:
        role = "source"
    return Placement(role, area=area, thing=thing, thing_folder=thing_folder, matched=bool(ids))


def unmatched_folders(placed: list[tuple[DeviceDoc, Placement]]) -> list[str]:
    """Level-2 folders the registry does not know, as `Area/Folder`, sorted."""
    return sorted({
        f"{p.area}/{p.thing_folder}"
        for _, p in placed
        if p.area and p.thing_folder and not p.matched
    })
