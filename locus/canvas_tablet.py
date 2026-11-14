"""The mirror to the tablet (spec P, step P4; P-D2, P-D3, P-D4, P-D5).

This year's Canvas documents go to `Studies/<course folder>/<module folder>/` on the tablet, under
their readable Canvas name ("B14_IP_MT26_L3"), except:
- **solutions** (P-D3): a name with "solution", "answer" or "soln" stays off, so a worked sheet
  never sits next to its answers; `~/files` and the library have them;
- code, data and anything not a document (P-D2);
- a module he is not taking (`taxonomy/canvas-skip.toml`).

**Never a duplicate** (his words, 14 Nov: "make sure only what is not already there is sent, I do
not want duplicates"). A file is sent only when BOTH hold:
- its bytes are nowhere on the tablet, in any folder: the sha256 of every PDF on the device is read
  from rmapi's tree (one `ls /`), the same hash his hand uploads matched by in P2 (24 of 24);
- locus has not sent it before: each send is recorded in the mirror's manifest (`tablet`), so a
  document he deletes from the tablet is not sent back.
A PowerPoint or Word file goes as a PDF converted on the server (LibreOffice); its converted bytes
differ run to run, so for those the manifest record is the guard.

A Canvas file is a SOURCE, not Claude's document: it is pushed with `authored=False`, so it is not
in `device_sent`, and capture reads it like a hand upload: its bytes are already in the library
(P3), and his marks on it key to that library document by content hash.

A name already taken in the folder by different bytes (Canvas revised a file he has) is not
replaced: the new one arrives beside it as "<name> (v2)" (P-D5), and his ink is never touched.

The reMarkable cloud rate-limits (HTTP 429 after six sends in a row, 14 Nov): sends are paced and
back off.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

from locus.canvas_files import _skipped, active_folders, skipped_modules
from locus.canvas_pull import MIRROR, _save_manifest, load_manifest

TABLET_TYPES = (".pdf", ".pptx", ".docx")
SOLUTION = re.compile(r"solution|answer|soln", re.I)


@dataclass
class Send:
    key: str          # manifest key (Canvas file id)
    src: str          # path in the mirror
    folder: str       # tablet folder
    title: str        # tablet document name


@dataclass
class TabletPlan:
    sends: list[Send] = field(default_factory=list)
    on_tablet: list[str] = field(default_factory=list)     # same bytes already there (his or ours)
    solutions: list[str] = field(default_factory=list)
    sent_before: int = 0


def title_of(display: str) -> str:
    stem, dot, ext = display.strip().rpartition(".")
    return stem if dot and re.fullmatch(r"[A-Za-z0-9]{1,6}", ext) else display.strip()


def plan(manifest: dict, courses: dict[str, dict], tablet_hashes: set[str],
         skip: dict[str, set[str]] | None = None) -> TabletPlan:
    live = active_folders(courses)
    skip = skipped_modules() if skip is None else skip
    p = TabletPlan()
    seen: set[str] = set()       # the same bytes twice on Canvas go once
    sent_shas = {e["tablet"] for e in manifest["files"].values() if e.get("tablet")}
    for key, e in sorted(manifest["files"].items(), key=lambda kv: kv[1]["path"]):
        if e["course"] not in live or e["status"] != "ok" or _skipped(e, skip) \
                or not e["path"].lower().endswith(TABLET_TYPES):
            continue
        if SOLUTION.search(e["name"]):
            p.solutions.append(e["path"])
            continue
        if e.get("tablet") == e["sha256"] or e["sha256"] in sent_shas:
            p.sent_before += 1
            continue
        if e["sha256"] in tablet_hashes or e["sha256"] in seen:
            p.on_tablet.append(e["path"])
            continue
        seen.add(e["sha256"])
        folder = "Studies/" + e["path"].rsplit("/", 1)[0]
        p.sends.append(Send(key, e["path"], folder, title_of(e["name"])))
    return p


def _to_pdf(src: Path, out_dir: Path) -> Path:
    if src.suffix.lower() == ".pdf":
        return src
    r = subprocess.run(["soffice", "--headless", "--convert-to", "pdf", "--outdir", str(out_dir), str(src)],
                       capture_output=True, text=True, timeout=300)
    pdf = out_dir / (src.stem + ".pdf")
    if r.returncode != 0 or not pdf.exists():
        raise RuntimeError(f"LibreOffice could not convert {src.name}: {(r.stderr or r.stdout).strip()[:200]}")
    return pdf


def _is_429(err: Exception) -> bool:
    return "429" in str(err) or "too many" in str(err).lower()


def send(p: TabletPlan, *, deliver, root: Path = MIRROR, pause: float = 10.0, log=print,
         sleep=time.sleep) -> tuple[int, list[str]]:
    """Push each planned file. `deliver(pdf_path, folder)` uploads one (raising on failure). Each
    success is written to the manifest at once, so an interrupted run never sends twice."""
    sent, errors = 0, []
    for i, s in enumerate(p.sends):
        man = load_manifest(root)
        e = man["files"][s.key]
        with tempfile.TemporaryDirectory() as td:
            tdir = Path(td)
            try:
                pdf = _to_pdf(root / s.src, tdir)
            except (RuntimeError, OSError, subprocess.SubprocessError) as err:
                errors.append(f"{s.src}: {err}")
                continue
            named = tdir / "named"
            named.mkdir()
            delay = 60.0
            for title in (s.title, f"{s.title} (v2)", f"{s.title} (v3)"):
                target = named / f"{title}.pdf"
                shutil.copyfile(pdf, target)
                try:
                    for attempt in range(4):
                        try:
                            deliver(target, s.folder)
                            break
                        except RuntimeError as err:
                            if not _is_429(err) or attempt == 3:
                                raise
                            log(f"429 from the cloud; waiting {delay:.0f}s")
                            sleep(delay)
                            delay *= 2
                    e["tablet"] = e["sha256"]
                    e["tablet_name"] = f"{s.folder}/{title}"
                    _save_manifest(man, root)
                    sent += 1
                    log(f"sent {s.folder}/{title}")
                    break
                except RuntimeError as err:
                    target.unlink()
                    if "already exists" in str(err):
                        continue        # the name is taken by other bytes: beside it, as (v2)
                    errors.append(f"{s.src}: {err}")
                    break
            else:
                errors.append(f"{s.src}: {s.title}, (v2) and (v3) all taken")
        if pause and i < len(p.sends) - 1:
            sleep(pause)
    return sent, errors


def tablet_hashes() -> set[str]:
    """sha256 of every PDF on the tablet, after one `rmapi ls /` brings the tree up to date."""
    from locus.capture.device import read_device
    from locus.capture.remarkable import _subprocess_runner
    from locus.config import load

    cfg = load()
    docs = read_device(_subprocess_runner(cfg.capture.rmapi_binary), tree_cache=cfg.capture.rmapi_tree_cache)
    return {d.pdf_hash for d in docs if d.pdf_hash}


def live_deliver():
    from locus.capture.remarkable import _subprocess_runner
    from locus.config import load
    from locus.reading.deliver_remarkable import _ensure_folder, deliver_pdf

    cfg = load()
    runner = _subprocess_runner(cfg.capture.rmapi_binary)
    made: set[str] = set()

    def deliver(pdf: Path, folder: str) -> None:
        if folder not in made:          # every level, so a new module folder exists first
            parts = folder.split("/")
            for n in range(1, len(parts) + 1):
                _ensure_folder(runner, "/" + "/".join(parts[:n]))
            made.add(folder)
        deliver_pdf(pdf, remote_folder="/" + folder, rmapi_binary=cfg.capture.rmapi_binary,
                    authored=False, runner=runner)

    return deliver
