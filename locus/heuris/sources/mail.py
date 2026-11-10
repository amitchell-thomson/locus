"""Mail items from the pushed thread files (H1.4): one item per message, cleaned.

The laptop writes `data/heuris-in/mail/<account>/<hash>.json`, one per thread, already kept by
the rule and scrubbed (locus/heuris/mail_push.py). Here each message becomes an item:

* `text` is the sender's name and the message with quoted replies and signatures removed. A
  reply quotes the whole thread below it; indexed as is, every message in a 30-message thread
  would carry the first one 29 times, and search would return the same paragraph from 29 items.
* `thing` comes from keryx's filing links (a thread filed under `millennium`), `area` from the
  thing, or from a folder link.
* `people` are the addresses and names on it, which is what `search(person=...)` and
  `timeline(person)` read.

The scan is full (every pushed file every run), so it prunes: a thread the rule stopped keeping
leaves the index on the next run.
"""

from __future__ import annotations

import json
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterator

from locus.heuris import registry
from locus.heuris.index import Item

# Where a reply's own text ends and the quoted thread begins. The first match wins.
_QUOTE_STARTS = [
    re.compile(r"^\s*On [^\n]{0,300}(?:\n[^\n]{0,200})?\bwrote:\s*$", re.M),  # Gmail, Apple Mail
    re.compile(r"^\s*-{2,}\s*Original Message\s*-{2,}", re.M | re.I),
    re.compile(r"^\s*_{8,}\s*$\n+\s*From:", re.M),                          # Outlook web
    re.compile(r"^\s*From: [^\n]+\n\s*(?:Sent|Date): [^\n]+\n", re.M),    # Outlook desktop
    re.compile(r"^\s*-{2,}\s*Forwarded message\s*-{2,}", re.M | re.I),
    re.compile(r"^\s*Begin forwarded message:", re.M),
]
_SIGNATURE_STARTS = [
    re.compile(r"^-- ?$", re.M),
    re.compile(r"^\s*Sent from my (?:iPhone|iPad|Android|Galaxy|mobile)[^\n]*$", re.M | re.I),
    re.compile(r"^\s*Get Outlook for (?:iOS|Android)[^\n]*$", re.M | re.I),
    re.compile(r"^\s*Sent from Outlook for[^\n]*$", re.M | re.I),
]


def clean(body: str, *, keep_forward: bool = True) -> str:
    """The message's own text: quoted replies and signatures removed, blank runs collapsed.

    A forward is the point of the message ("Fw: Gavekal Daily"), so with `keep_forward` the
    forwarded part is kept when the note above it is short. A body that is ALL quote (top-posted
    nothing) keeps its first 1,000 characters rather than becoming empty.
    """
    text = body.replace("\r\n", "\n")
    forwarded = ""
    cut = min((m.start() for p in _QUOTE_STARTS if (m := p.search(text))), default=None)
    if cut is not None:
        own = text[:cut]
        # A forward is the point of the message: keep it when the note above it is short (the
        # "Fwd: Verbier" sample kept only "Kind Regards, Callum" and lost the trip, 10 Nov).
        # Apple Mail quotes a forward with "> ", so it is unquoted, not dropped with the quotes.
        if keep_forward and len(own.strip()) < 300 and re.search(r"forward", text[cut:cut + 80], re.I):
            forwarded = re.sub(r"^ ?> ?", "", text[cut:cut + 8000], flags=re.M)
        text = own if own.strip() or forwarded else body[:1000]
    unquoted = "\n".join(line for line in text.split("\n") if not line.lstrip().startswith(">"))
    if not unquoted.strip() and text.strip():
        # Everything quoted: some clients send a reply with the writer's own text prefixed "> "
        # (Alec's 5 Nov reply to Millennium, with the passport line, was indexed EMPTY; H1 fix
        # 4). The message's own text is then its first quoted block, up to the quoted "On …
        # wrote:" that starts the thread below it.
        first = re.sub(r"^ ?> ?", "", text, flags=re.M)
        stop = min((m.start() for q in _QUOTE_STARTS if (m := q.search(first))), default=len(first))
        unquoted = "\n".join(line for line in first[:stop].split("\n") if not line.lstrip().startswith(">"))
    text = unquoted
    sig = min((m.start() for p in _SIGNATURE_STARTS if (m := p.search(text))), default=None)
    if sig is not None and sig > 0:
        text = text[:sig]
    if forwarded:
        text = f"{text.strip()}\n\n{forwarded}"
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def _when(ts) -> datetime:
    return datetime.fromtimestamp(int(ts), tz=timezone.utc)


def _addr(x) -> tuple[str, str]:
    if isinstance(x, dict):
        return (x.get("email") or "").lower(), (x.get("name") or "").strip()
    return "", ""


def items(root: Path) -> Iterator[Item]:
    for f in sorted(root.glob("*/*.json")):
        try:
            doc = json.loads(f.read_text())
        except (OSError, ValueError):
            continue
        acct, thread = doc["account"], doc["thread"]
        thing = next((ln["target"] for ln in doc.get("links", []) if ln.get("kind") == "item"), None)
        area = registry.area_of(thing) or next(
            (ln["target"] for ln in doc.get("links", []) if ln.get("kind") == "folder"), None)
        needs_reply = any(t.get("kind") == "needs_reply" for t in doc.get("tags", []))
        for m in doc.get("messages", []):
            email, name = _addr(m.get("from"))
            people = [(email, "from"), (name, "name")]
            for role in ("to", "cc"):
                for x in m.get(role) or []:
                    e, n = _addr(x)
                    people += [(e, role), (n, "name")]
            own = clean(m.get("body") or "")
            yield Item(
                ref=f"mail:{acct}/{thread}/{m['id']}",
                kind="mail",
                at=_when(m["date"]),
                title=(m.get("subject") or "").strip(),
                text=f"{name or email}: {own}" if own else (name or email),
                area=area,
                thing=thing,
                parent=f"mail:{acct}/{thread}",
                people=[p for p in people if p[0]],
                meta={"account": acct, "from": email, "from_name": name, "why": m.get("why"),
                      "flagged": m.get("flagged"), "needs_reply": needs_reply,
                      "attachments": m.get("attachments") or [], "in_inbox": m.get("in_inbox")},
            )
            # Attachments read on the laptop (H1.12): one item each, beside their message, with
            # its people, thing and area so every filter that finds the message finds them.
            for i, d in enumerate(m.get("docs") or []):
                yield Item(
                    ref=f"attachment:{acct}/{thread}/{m['id']}/{i}",
                    kind="attachment",
                    at=_when(m["date"]),
                    title=f"{d['name']} (attached to \"{(m.get('subject') or '').strip()}\")",
                    text=d["text"],
                    area=area,
                    thing=thing,
                    parent=f"mail:{acct}/{thread}",
                    people=[p for p in people if p[0]],
                    meta={"account": acct, "from": email, "from_name": name, "name": d["name"],
                          "sha": d.get("sha"), "message": f"mail:{acct}/{thread}/{m['id']}"},
                )


def record_rule(root: Path) -> None:
    """What the laptop's rule left out, into `locus gates` (gate `heuris.mail_rule`)."""
    try:
        summary = json.loads((root / "_rule.json").read_text())
    except (OSError, ValueError):
        return
    from locus.config import load
    from locus.db.connection import get_connection
    from locus.observe import gates

    conn = get_connection(load().paths.db)
    try:
        for why, samples in summary.get("left_out_samples", {}).items():
            for s in samples[:3]:
                gates.record(conn, "heuris.mail_rule", rejected=True, value=f"{why}: {s}")
        conn.commit()
    finally:
        conn.close()
