"""Loop C — capture Claude conversations into the note pipeline (agent-layer plan §8.3).

Much of the owner's reasoning and decisions happen in conversations with Claude. Loop C turns a
chosen conversation (or a decision-summary of it) into a `rough` note, so that reasoning becomes
searchable, linked corpus — the same as handwriting (Loop A), minus the vision step.

Three entry points feed this one core writer:
  - the MCP `capture` tool (cross-client — Claude Code, claude.ai, phone; mcp_server.py),
  - the `/locus-capture` Claude Code command (summarise-then-capture),
  - `import_jsonl_transcript` — pull a Claude Code `.jsonl` transcript from disk retroactively.

**Write-to-inbox ONLY.** Capture writes a markdown file into `notes/conversations/`; it never
touches the corpus directly. The note is `maturity=rough`, `category=note`, and is picked up by
the normal note-sync (③) on the next run — so Loop C inherits idempotency, replace-by-path, and the
rough down-weight for free, and stays invariant-clean (one direction per boundary).
"""

from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from locus.config import load
from locus.vault.writer import _atomic_write

CONVERSATIONS_SUBDIR = "conversations"


@dataclass
class ConversationCapture:
    path: Path
    slug: str
    title: str


def conversations_dir(notes_dir: str | Path | None = None) -> Path:
    base = Path(notes_dir) if notes_dir is not None else load().paths.notes
    return base / CONVERSATIONS_SUBDIR


def _slugify(title: str, *, maxlen: int = 60) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")[:maxlen].strip("-")
    return s or "conversation"


def _frontmatter(title: str, project: str | None, source: str, now: str) -> str:
    fields = {
        "title": title,
        "category": "note",     # coarse KIND (a conversation is a rough note); read by note-sync
        "maturity": "rough",
        "source": source,       # provenance: where this capture came from
        "captured": now,
    }
    if project:
        fields["project"] = project
    return "---\n" + "\n".join(f"{k}: {v}" for k, v in fields.items()) + "\n---\n"


def capture_conversation(
    content: str,
    *,
    title: str,
    project: str | None = None,
    source: str = "conversation",
    notes_dir: str | Path | None = None,
    now: str | None = None,
) -> ConversationCapture:
    """Write a conversation capture to `notes/conversations/<slug>-<hash>.md` (rough). Returns it.

    The filename carries a short content hash so distinct captures never clobber each other while a
    re-capture of identical content stays idempotent (same file). Does NOT ingest — note-sync does."""
    now = now or datetime.now(timezone.utc).isoformat()
    body = content.strip()
    digest = hashlib.sha256(body.encode("utf-8")).hexdigest()[:8]
    slug = f"{_slugify(title)}-{digest}"
    path = conversations_dir(notes_dir) / f"{slug}.md"
    _atomic_write(path, _frontmatter(title, project, source, now) + "\n" + body + "\n")
    return ConversationCapture(path=path, slug=slug, title=title)


# --- batch importer: Claude Code .jsonl transcript ---------------------------------------------

def _extract_text(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            b.get("text", "") for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        )
    return ""


def _parse_jsonl(path: Path) -> list[tuple[str, str]]:
    """(role, text) for each user/assistant turn in a Claude Code .jsonl transcript; tool/meta
    lines and empty turns are skipped."""
    turns: list[tuple[str, str]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        message = obj.get("message") or {}
        role = message.get("role") or obj.get("type")
        if role not in ("user", "assistant"):
            continue
        text = _extract_text(message.get("content")).strip()
        if text:
            turns.append((role, text))
    return turns


def _format_transcript(turns: list[tuple[str, str]]) -> str:
    blocks = [f"**{'You' if role == 'user' else 'Claude'}:**\n\n{text}" for role, text in turns]
    return "\n\n---\n\n".join(blocks)


def import_jsonl_transcript(
    jsonl_path: str | Path,
    *,
    title: str | None = None,
    project: str | None = None,
    notes_dir: str | Path | None = None,
) -> ConversationCapture:
    """Import a Claude Code `.jsonl` transcript as a rough conversation note. `title` defaults to
    the first user line (truncated) or the file stem."""
    path = Path(jsonl_path)
    turns = _parse_jsonl(path)
    if not turns:
        raise ValueError(f"no user/assistant turns found in {path}")
    if title is None:
        first_user = next((t for r, t in turns if r == "user"), path.stem)
        title = " ".join(first_user.split())[:70] or path.stem
    return capture_conversation(
        _format_transcript(turns), title=title, project=project,
        source="claude-code", notes_dir=notes_dir,
    )
