"""Repository docs and commits reach the server from the laptop (H1.7).

The laptop is the one place every project's repositories live: oqts is eight repositories in one
folder, gr-ai-ml is notes with no git, cv has no remote, and the server has clones of only some
(measured 10 Nov). So each registry project's folder under `~/local-projects/` is read HERE,
scrubbed, and pushed beside the mail as one JSON file per project.

Read (heuris C: "READMEs, docs, decision logs. Code is not indexed"):

* every `.md`, and `.tex` / `.txt` under a `docs/` folder or at the top level (cv.tex is the CV);
* every commit's subject, body, date and the files it touched, from each git repository inside
  the folder (up to two levels down), for the last three years.

Skipped: build and dependency folders, data, and any file over 300 KB (a generated report or a
dumped log is not a doc).
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from locus.heuris import registry
from locus.scrub import load_known, scrub
from locus.transcripts_push import state_dir

PROJECTS = Path(os.environ.get("LOCUS_PROJECTS_DIR", "~/local-projects")).expanduser()
SKIP_DIRS = {".git", "node_modules", "target", ".venv", "venv", "__pycache__", "build", "dist", "data",
             ".mypy_cache", ".pytest_cache", "vendor", ".next", "site-packages", "runs", "outputs",
             "fixtures", ".claude", "archive", "logs", "cache", ".cache"}
MAX_DOC = 300_000
SINCE = "3 years ago"


@dataclass
class Report:
    projects: int = 0
    docs: int = 0
    commits: int = 0
    written: int = 0
    missing: list[str] = field(default_factory=list)
    redacted: dict[str, int] = field(default_factory=dict)
    sent: bool = False
    error: str = ""


def folder_of(entry: dict) -> Path | None:
    for name in [entry.get("folder"), *entry.get("sessions", [])]:
        if name and (PROJECTS / name).is_dir():
            return PROJECTS / name
    return None


def _walk(root: Path):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")]
        for f in filenames:
            yield Path(dirpath) / f


def docs(root: Path) -> list[Path]:
    out = []
    for p in _walk(root):
        rel = p.relative_to(root)
        ext = p.suffix.lower()
        top_or_docs = len(rel.parts) == 1 or "docs" in rel.parts
        if ext == ".md" or (ext in (".tex", ".txt") and top_or_docs):
            try:
                if p.stat().st_size <= MAX_DOC:
                    out.append(p)
            except OSError:
                pass
    return sorted(out)


def git_repos(root: Path) -> list[Path]:
    found = [root] if (root / ".git").exists() else []
    for child in sorted(root.iterdir()) if root.is_dir() else []:
        if child.is_dir() and child.name not in SKIP_DIRS:
            if (child / ".git").exists():
                found.append(child)
            else:
                found += [g for g in sorted(child.iterdir()) if g.is_dir() and (g / ".git").exists()] if not child.name.startswith(".") else []
    return found


def commits(repo: Path) -> list[dict]:
    fmt = "%x1e%H%x1f%aI%x1f%an%x1f%s%x1f%b%x1f"
    try:
        out = subprocess.run(["git", "-C", str(repo), "log", "--all", f"--since={SINCE}", f"--format={fmt}",
                              "--name-only", "--no-renames"], capture_output=True, text=True, timeout=120).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    res = []
    for rec in out.split("\x1e")[1:]:
        parts = rec.split("\x1f")
        if len(parts) < 6:
            continue
        sha, at, author, subject, body, files = parts[:6]
        res.append({"sha": sha, "at": at, "author": author, "subject": subject.strip(), "body": body.strip(),
                    "files": [f for f in files.strip().splitlines() if f][:40]})
    return res


def build(stage: Path | None = None) -> Report:
    r = Report()
    stage = stage or state_dir() / "repos"
    stage.mkdir(parents=True, exist_ok=True)
    known = load_known()

    def clean(text: str) -> str:
        t, c = scrub(text, known)
        for k, v in c.items():
            r.redacted[k] = r.redacted.get(k, 0) + v
        return t

    wanted = set()
    for pid, entry in registry.entries().items():
        if entry.get("kind") != "repo":
            continue
        root = folder_of(entry)
        if root is None:
            r.missing.append(pid)
            continue
        r.projects += 1
        doc_rows = []
        for p in docs(root):
            try:
                text = p.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            doc_rows.append({"path": str(p.relative_to(root)), "mtime": p.stat().st_mtime, "text": clean(text)})
        commit_rows = []
        for g in git_repos(root):
            sub = "" if g == root else str(g.relative_to(root))
            for c in commits(g):
                c["repo"] = sub
                c["subject"], c["body"] = clean(c["subject"]), clean(c["body"])
                commit_rows.append(c)
        r.docs += len(doc_rows)
        r.commits += len(commit_rows)
        f = stage / f"{pid}.json"
        wanted.add(f)
        doc = json.dumps({"id": pid, "folder": root.name, "docs": doc_rows, "commits": commit_rows},
                         ensure_ascii=False, sort_keys=True)
        if f.exists() and hashlib.sha256(f.read_bytes()).hexdigest() == hashlib.sha256(doc.encode()).hexdigest():
            continue
        tmp = f.with_suffix(".part")
        tmp.write_text(doc)
        os.chmod(tmp, 0o600)
        tmp.replace(f)
        r.written += 1
    for f in stage.glob("*.json"):
        if f not in wanted:
            f.unlink()
    # The applications file lives in ~/files on the laptop only; `context` on the server reads
    # this copy (it is his own record, not a secret, and small).
    apps = Path("~/files/Careers/applications.toml").expanduser()
    if apps.exists():
        (stage / "_applications.toml").write_text(apps.read_text())
    return r


def push(send: bool = True) -> Report:
    stage = state_dir() / "repos"
    r = build(stage)
    if not send:
        return r
    host = os.environ.get("LOCUS_HOST", "compute-node")
    rd = f"{os.environ.get('LOCUS_DIR', '/home/alec/server-projects/locus')}/data/heuris-in/repos/"
    proc = subprocess.run(
        ["rsync", "-a", "--delete", "--chmod=Du=rwx,Dgo=,Fu=rw,Fgo=", "-e",
         "ssh -o ConnectTimeout=10 -o BatchMode=yes", "--rsync-path", f"mkdir -p {rd} && rsync",
         f"{stage}/", f"{host}:{rd}"],
        capture_output=True, text=True, timeout=1800,
    )
    r.sent = proc.returncode == 0
    if not r.sent:
        r.error = (proc.stderr.strip().splitlines() or ["rsync failed"])[-1]
    return r


def describe(r: Report) -> str:
    miss = f"; no folder for {', '.join(r.missing)}" if r.missing else ""
    sent = "sent to the server" if r.sent else (f"NOT sent: {r.error}" if r.error else "not sent")
    return f"{r.projects} projects, {r.docs} docs, {r.commits} commits, {r.written} files written{miss}; {sent}"
