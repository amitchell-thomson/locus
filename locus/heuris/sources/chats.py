"""Claude chat items from transcripts on the server (H1.6).

Two roots: the laptop's transcripts, pushed and scrubbed by 0.10 into `data/transcripts/laptop/`,
and the server's own `~/.claude/projects/`. Each project folder's top-level `<session>.jsonl` is a
session; subfolders (subagent runs, tool-result spill files) are not read.

What becomes an item (heuris C: "your turns as written; Claude's answers in chunks"):

* each of your prompts, whole (a paste is chunked by the indexer like anything else), with
  `<system-reminder>` blocks and slash-command plumbing removed;
* each of Claude's replies: the prose text between one prompt of yours and the next, joined.
  Tool calls, tool output and thinking are not indexed: they are most of the 1.1 GB and almost
  none of the meaning, and `open` shows the turns around a hit when the detail matters.

Skipped: compaction summaries (model prose about the chat, not the chat), meta lines, sidechains,
and the server's headless `-tmp` sessions, which are locus's own jobs; indexing them would have
locus read itself (heuris C).

`thing` is the registry entry the session's folder belongs to (`~/local-projects/quast` is
quast; `~/files/Projects/gr-ai-ml` is gr-ai-ml).

Only files whose size or mtime moved since the last run are read (`file_state`), so the index is
not pruned from here: a deleted transcript's items stay, which is harmless.
"""

from __future__ import annotations

import json
import re
import sqlite3
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterator

from locus.heuris import registry
from locus.heuris.index import Item

SOURCE = "chats"
_REMINDER = re.compile(r"<system-reminder>.*?</system-reminder>", re.S)
_PLUMBING = ("<command-name>", "<command-message>", "<local-command-stdout>", "<local-command-caveat>",
             "Caveat: The messages below", "[Request interrupted")
MAX_PROMPT = 20_000


def roots(locus_dir: Path) -> list[tuple[str, Path]]:
    return [("laptop", locus_dir / "data" / "transcripts" / "laptop"),
            ("server", Path("~/.claude/projects").expanduser())]


def thing_of(cwd: str | None) -> str | None:
    if not cwd:
        return None
    parts = Path(cwd).parts
    for anchor in ("local-projects", "server-projects"):
        if anchor in parts:
            i = parts.index(anchor)
            if i + 1 < len(parts):
                return registry.by_folder(parts[i + 1])
    if "files" in parts:
        i = parts.index("files")
        if i + 2 < len(parts):
            return registry.by_folder(parts[i + 2])
    return None


def _text_of(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n\n".join(b.get("text", "") for b in content
                           if isinstance(b, dict) and b.get("type") == "text")
    return ""


def _prompt(content) -> str:
    if isinstance(content, list) and any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content):
        return ""
    t = _REMINDER.sub("", _text_of(content)).strip()
    if not t or t.startswith(_PLUMBING):
        return ""
    return t[:MAX_PROMPT]


def _ts(s: str | None) -> datetime:
    if not s:
        return datetime.now(timezone.utc)
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def session_items(path: Path, machine: str) -> list[Item]:
    session = path.stem
    out: list[Item] = []
    title, cwd = None, None
    reply: list[str] = []
    reply_at: str | None = None
    reply_uuid: str | None = None
    n = 0

    def flush() -> None:
        nonlocal reply, reply_at, reply_uuid, n
        text = "\n\n".join(t for t in reply if t.strip()).strip()
        if text and reply_uuid:
            n += 1
            out.append(Item(ref=f"chat:{machine}/{session}#{reply_uuid}", kind="chat", at=_ts(reply_at),
                            title="Claude: " + text.splitlines()[0][:100], text=text,
                            parent=f"chat:{machine}/{session}",
                            meta={"role": "claude", "turn": n}))
        reply, reply_at, reply_uuid = [], None, None

    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    for line in lines:
        try:
            d = json.loads(line)
        except ValueError:
            continue
        t = d.get("type")
        if t == "ai-title":
            title = d.get("aiTitle") or d.get("title") or title
            continue
        if t not in ("user", "assistant") or d.get("isSidechain") or d.get("isMeta") or d.get("isCompactSummary"):
            continue
        cwd = d.get("cwd") or cwd
        content = (d.get("message") or {}).get("content")
        if t == "user":
            p = _prompt(content)
            if not p:
                continue
            flush()
            n += 1
            out.append(Item(ref=f"chat:{machine}/{session}#{d.get('uuid')}", kind="chat", at=_ts(d.get("timestamp")),
                            title=p.splitlines()[0][:100], text=p, parent=f"chat:{machine}/{session}",
                            meta={"role": "you", "turn": n}))
        else:
            txt = _text_of(content)
            if txt.strip():
                reply.append(txt)
                reply_at = reply_at or d.get("timestamp")
                reply_uuid = reply_uuid or d.get("uuid")
    flush()
    thing = thing_of(cwd)
    for it in out:
        it.thing = thing
        it.area = registry.area_of(thing)
        it.meta.update({"session": session, "machine": machine, "cwd": cwd, "session_title": title})
    return out


# Held out of the index while the H1 test set is scored (H1.11): this session wrote the answer key
# and read the answers out of his files (the questionnaire's ticks among them), so a scoring chat
# that found it would be marking its own homework. Removed from the index too (`purge_held_out`).
HELD_OUT_SESSIONS = {"81c0a62a-3b15-4f89-ae5c-80227ae38eab"}


def purge_held_out(conn: sqlite3.Connection) -> int:
    from locus.heuris.index import _delete_item

    ids = [r[0] for sid in HELD_OUT_SESSIONS for r in conn.execute(
        "SELECT id FROM items WHERE source=? AND parent LIKE ?", (SOURCE, f"chat:%/{sid}")).fetchall()]
    with conn:
        for i in ids:
            _delete_item(conn, i)
    return len(ids)


def changed_files(conn: sqlite3.Connection, locus_dir: Path) -> list[tuple[str, Path, int, float]]:
    seen = {(p): (s, m) for p, s, m in conn.execute("SELECT path, size, mtime FROM file_state WHERE source=?", (SOURCE,))}
    out = []
    for machine, root in roots(locus_dir):
        if not root.exists():
            continue
        for proj in sorted(root.iterdir()):
            if not proj.is_dir() or proj.name.endswith("-tmp") or "-tmp-" in proj.name:
                continue
            for f in sorted(proj.glob("*.jsonl")):
                if f.stem in HELD_OUT_SESSIONS:
                    continue
                st = f.stat()
                if seen.get(str(f)) != (st.st_size, st.st_mtime):
                    out.append((machine, f, st.st_size, st.st_mtime))
    return out


def mark_read(conn: sqlite3.Connection, files: list[tuple[str, Path, int, float]]) -> None:
    with conn:
        conn.executemany("INSERT OR REPLACE INTO file_state(source, path, size, mtime) VALUES (?,?,?,?)",
                         [(SOURCE, str(f), s, m) for _, f, s, m in files])


def items(files: list[tuple[str, Path, int, float]]) -> Iterator[Item]:
    for machine, f, _, _ in files:
        yield from session_items(f, machine)
