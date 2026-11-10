"""Mail reaches the server: kept, scrubbed, one file per thread, every 15 minutes (H1.4, D2, D3).

keryxd runs only on the Mac, so the only full copy of the mail is keryx's mirror there
(`~/.local/share/keryx/keryx.db`, 7,677 messages on 10 Nov). This reads it read-only, keeps
what the rule below keeps, scrubs every kept message (locus/scrub.py) and writes one JSON file
per thread into a staging tree, which rsync mirrors to the server's `data/heuris-in/mail/`.

**The rule** (H1.2, set on your real mail on 10 Nov; your answers are in the spec):

* OQTS-shared never leaves the laptop. It is excluded here, before anything is written.
* Kept: what you sent; threads you replied to; flagged mail; firms and recruiting systems;
  people you have written to; mail from a person; every Oxford list and office ("all Oxford
  lists"); travel bookings (not shop receipts); quant and careers societies.
* Left out: other lists and automated mail (list headers, `noreply`-style senders, bulk-mail
  domains), and companies with no person behind them.
* The careers service's "Daily Opportunities Alert" is left out although it is an Oxford list:
  144 job digests, kept only because you once wrote to that address (fixed without asking, H1.2).

What the rule left out is written to `_rule.json` with samples, and the server records it in
`locus gates` (gate `heuris.mail_rule`), so a rule that drops something it should not shows up
in words rather than as a missing answer (CLAUDE.md §3; test question 44).

**Attachments** (H1.12): each PDF, Word or text attachment on a kept message is read from Mail's
own store and redacted (locus/heuris/documents.py has what is never read and why), and travels
with its message as `docs`. One Mail has not downloaded is counted, not fetched.

A thread whose kept set is empty has no file; `rsync --delete` then removes it on the server and
the indexer's full scan prunes it, so tightening the rule takes mail out of the index too.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sqlite3
import subprocess
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

from locus.heuris import documents
from locus.scrub import load_known, scrub
from locus.transcripts_push import state_dir

KERYX_DB = Path(os.environ.get("KERYX_DB", "~/.local/share/keryx/keryx.db")).expanduser()
EXCLUDED_ACCOUNTS = {"oqts-shared"}
ME = {"alec.mitchellthomson@gmail.com", "newc7370@ox.ac.uk", "alec.mitchell-thomson@new.ox.ac.uk",
      "alec@oqts.org"}

FIRMS = ("mlp.com", "brevanhoward", "jumptrading", "citadel", "gresearch", "janestreet", "hrt.com",
         "hudson-trading", "qrt.com", "tower-research", "optiver", "imc.com", "sig.com", "susquehanna",
         "drw.com", "xtx", "five-rings", "deshaw", "twosigma", "jpmorgan", "jpmchase", "citi.com",
         "goldmansachs", "gs.com", "morganstanley", "bankofamerica", "barclays")
ATS = ("workday.com", "greenhouse", "lever.co", "hirevue", "eightfold", "hackerrank", "codility",
       "criteriacorp", "ashbyhq", "avature", "oleeo", "smartrecruiters", "icims", "taleo",
       "successfactors", "beapplied", "pymetrics")
TRAVEL = ("ba.com", "britishairways", "easyjet", "ryanair", "trainline", "lner", "gwr.com",
          "avantiwestcoast", "nationalrail", "eurostar", "booking.com", "airbnb", "expedia",
          "hotels.com", "lufthansa", "klm", "airfrance", "bahn.de", "rheinbahn", "sncf", "jet2",
          "wizzair", "virginatlantic", "emirates", "qatarairways", "chilternrailways", "crosscountry",
          "thameslink", "southernrailway", "nationalexpress", "megabus", "oxfordbus", "flixbus")
TRAVEL_SUBJECT = re.compile(r"ticket|booking|itinerary|boarding|reservation|confirmation|journey|"
                            r"flight|check.?in|receipt|bestätigung|reise", re.I)
# A society, not anything with a finance word in it: the first version also kept GitHub bots on a
# trading repo, FT Unhedged and a Coutts newsletter (10 Nov samples).
SOCIETY = re.compile(r"alpha fund|\bOAF\b|quant\w* bootcamp|\bOQTS\b|quantitative trading society|"
                     r"(invest\w*|finance|fintech|trading|quant\w*|economics) society|\bOUIS\b|\bOUFS\b", re.I)
PERSONAL = {"gmail.com", "googlemail.com", "yahoo.co.uk", "yahoo.com", "hotmail.com", "hotmail.co.uk",
            "outlook.com", "icloud.com", "me.com", "live.co.uk", "btinternet.com"}
BOT_LOCAL = re.compile(r"^(no-?reply|do-?not-?reply|donotreply|notifications?|newsletters?|news|info|"
                       r"marketing|mailer|updates|support|team|hello|alerts?|digest|editor|"
                       r"subscriptions?|customerservices?|service|feedback|bounce|mailer-daemon|"
                       r"postmaster|confirm)\b", re.I)
BOT_HDR = {"list-unsubscribe", "list-id", "auto-submitted", "precedence", "feedback-id",
           "x-auto-response-suppress"}
# Words that make "Microsoft Family Safety" look like a first and last name to the name test.
NOT_A_NAME = {"microsoft", "google", "apple", "family", "safety", "team", "club", "society",
              "office", "services", "service", "support", "news", "events", "store", "college",
              "university", "school", "global", "summer", "online", "account", "accounts", "the"}
BULK_DOMAIN = re.compile(r"^(e?mail|messages?|news|em|e|info|marketing|notify|notifications)\.")


@dataclass
class Ctx:
    replied: set[tuple[str, str]]
    wrote_to: set[str]


def _headers(lh: str | None) -> dict[str, str]:
    try:
        v = json.loads(lh or "[]")
    except ValueError:
        return {}
    return {str(h[0]).lower(): str(h[1]) for h in v if isinstance(h, list) and len(h) == 2} if isinstance(v, list) else {}


def _looks_named(name: str, local: str) -> bool:
    m = re.match(r"^([A-Z][a-z'\-]+),? ([A-Z][a-z'\-]+)", name)
    if not m or {m.group(1).lower(), m.group(2).lower()} & NOT_A_NAME:
        return False
    return any(p.lower() in local for p in re.split(r"[ ,.]+", name) if len(p) > 2)


def keep(m: dict, ctx: Ctx) -> tuple[bool | None, str]:
    """(kept?, why). None = never leaves the laptop. `m` is one keryx `messages` row as a dict."""
    acct = m["account"]
    if acct in EXCLUDED_ACCOUNTS:
        return None, "OQTS-shared: never leaves the laptop"
    fr = json.loads(m.get("from_json") or "{}") or {}
    email = (fr.get("email") or "").lower()
    name = (fr.get("name") or "").strip()
    local, _, dom = email.partition("@")
    subject = m.get("subject") or ""
    hdrs = _headers(m.get("list_headers_json"))
    if email in ME:
        return True, "you sent it"
    if (acct, m["thread_id"]) in ctx.replied:
        return True, "a thread you replied to"
    if m.get("flagged"):
        return True, "you flagged it"
    if any(x in dom for x in FIRMS) or any(x in dom for x in ATS):
        return True, "a firm or recruiting system"
    if dom.startswith("careers.ox.ac.uk") and "daily opportunities alert" in subject.lower():
        return False, "careers service daily alert"
    if dom.endswith("ox.ac.uk") or "ox.ac.uk" in hdrs.get("list-id", ""):
        return True, "an Oxford list or office"
    if any(x in dom for x in TRAVEL) and TRAVEL_SUBJECT.search(subject):
        return True, "a travel booking"
    if not BOT_LOCAL.match(local) and (SOCIETY.search(name) or SOCIETY.search(hdrs.get("list-id", ""))):
        return True, "a quant or careers society"
    if set(hdrs) & BOT_HDR:
        return False, "list or automated headers"
    if email in ctx.wrote_to:
        return True, "someone you have written to"
    if BOT_LOCAL.match(local):
        return False, "automated sender"
    if BULK_DOMAIN.match(dom):
        return False, "bulk-mail domain"
    personal_dom = dom.endswith(("cam.ac.uk", "cantab.net")) or dom in PERSONAL
    named = _looks_named(name, local)
    if personal_dom and (named or not name):
        return True, "a person (personal address)"
    if named:
        return True, "a person (named sender)"
    return False, "a company with no person behind it"


def context(conn: sqlite3.Connection) -> Ctx:
    replied, wrote_to = set(), set()
    for acct, thread, fr, to, cc in conn.execute(
            "SELECT account, thread_id, from_json, to_json, cc_json FROM messages"):
        if ((json.loads(fr or "{}") or {}).get("email") or "").lower() in ME:
            replied.add((acct, thread))
            for blob in (to, cc):
                for x in json.loads(blob or "[]") or []:
                    if isinstance(x, dict) and x.get("email"):
                        wrote_to.add(x["email"].lower())
    return Ctx(replied, wrote_to)


@dataclass
class Report:
    kept: int = 0
    left_out: int = 0
    excluded: int = 0
    threads_written: int = 0
    threads_removed: int = 0
    reasons: Counter = field(default_factory=Counter)
    redacted: dict[str, int] = field(default_factory=dict)
    attachments_read: int = 0
    attachments_empty: int = 0
    attachments_skipped: Counter = field(default_factory=Counter)
    sent: bool = False
    error: str = ""


def _thread_file(stage: Path, account: str, thread: str) -> Path:
    return stage / account / (hashlib.sha1(thread.encode()).hexdigest()[:16] + ".json")


_COLS = ("id", "account", "thread_id", "message_id", "from_json", "to_json", "cc_json", "subject", "date",
         "in_inbox", "read", "flagged", "list_headers_json", "body_text", "attachments_json", "raw_ref")


def build(db: Path = KERYX_DB, stage: Path | None = None) -> Report:
    """Write the staging tree from keryx's mirror. Unchanged thread files are not rewritten."""
    r = Report()
    stage = stage or state_dir() / "mail"
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        ctx = context(conn)
        known = load_known()
        cache = documents.Cache(state_dir() / "documents-cache.json")
        threads: dict[tuple[str, str], list[dict]] = defaultdict(list)
        left_samples: dict[str, list[str]] = defaultdict(list)
        for row in conn.execute(f"SELECT {', '.join(_COLS)} FROM messages ORDER BY date"):
            m = dict(zip(_COLS, row))
            k, why = keep(m, ctx)
            r.reasons[(k, why)] += 1
            if k is None:
                r.excluded += 1
                continue
            if not k:
                r.left_out += 1
                if len(left_samples[why]) < 8:
                    fr = (json.loads(m["from_json"] or "{}") or {})
                    left_samples[why].append(f"{fr.get('email', '')} | {(m['subject'] or '')[:80]}")
                continue
            r.kept += 1
            body, counts = scrub(m["body_text"] or "", known)
            subj, c2 = scrub(m["subject"] or "", known)
            for c in (counts, c2):
                for kk, v in c.items():
                    r.redacted[kk] = r.redacted.get(kk, 0) + v
            atts = [a for a in (json.loads(m["attachments_json"] or "[]") or []) if isinstance(a, dict)]
            docs = []
            for a in atts:
                aname = a.get("name") or a.get("filename") or ""
                if not aname.lower().endswith(documents.READABLE):
                    continue  # images and the rest: named on the message, not read
                if why_not := documents.denied(aname):
                    r.attachments_skipped[why_not] += 1
                    continue
                data = documents.attachment_bytes(m["raw_ref"], a.get("id"), aname)
                if data is None:
                    r.attachments_skipped["not in Mail's store"] += 1
                    continue
                sha, text = cache.text(aname, data, known, r.redacted)
                if not text:
                    r.attachments_empty += 1
                    continue
                r.attachments_read += 1
                docs.append({"name": aname, "sha": sha, "text": text})
            threads[(m["account"], m["thread_id"])].append({
                "id": m["id"], "message_id": m["message_id"], "from": json.loads(m["from_json"] or "{}"),
                "to": json.loads(m["to_json"] or "[]"), "cc": json.loads(m["cc_json"] or "[]"),
                "subject": subj, "date": m["date"], "in_inbox": bool(m["in_inbox"]),
                "read": bool(m["read"]), "flagged": bool(m["flagged"]), "why": why, "body": body,
                "attachments": [a.get("name") or a.get("filename") for a in atts], "docs": docs,
            })
        tags = defaultdict(list)
        for acct, thread, kind, reason in conn.execute("SELECT account, thread_id, kind, reason FROM tags"):
            tags[(acct, thread)].append({"kind": kind, "reason": reason})
        links = defaultdict(list)
        for acct, thread, kind, target, title in conn.execute(
                "SELECT account, thread_id, kind, target, title FROM links WHERE app='locus'"):
            links[(acct, thread)].append({"kind": kind, "target": target, "title": title})
        cache.save()
    finally:
        conn.close()

    wanted: set[Path] = set()
    for (acct, thread), msgs in threads.items():
        f = _thread_file(stage, acct, thread)
        wanted.add(f)
        doc = json.dumps({"account": acct, "thread": thread, "tags": tags.get((acct, thread), []),
                          "links": links.get((acct, thread), []), "messages": msgs},
                         ensure_ascii=False, sort_keys=True)
        if f.exists() and f.read_text() == doc:
            continue
        f.parent.mkdir(parents=True, exist_ok=True)
        tmp = f.with_suffix(".part")
        tmp.write_text(doc)
        os.chmod(tmp, 0o600)
        tmp.replace(f)
        r.threads_written += 1
    for f in stage.glob("*/*.json"):
        if f not in wanted:
            f.unlink()
            r.threads_removed += 1
    summary = {"kept": r.kept, "left_out": r.left_out, "excluded": r.excluded,
               "reasons": [{"kept": k, "why": w, "n": n} for (k, w), n in r.reasons.most_common()],
               "left_out_samples": left_samples}
    (stage / "_rule.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1))
    return r


def remote_dir() -> str:
    d = os.environ.get("LOCUS_DIR", "/home/alec/server-projects/locus")
    return f"{d}/data/heuris-in/mail/"


def push(db: Path = KERYX_DB, send: bool = True) -> Report:
    stage = state_dir() / "mail"
    r = build(db, stage)
    if not send:
        return r
    host = os.environ.get("LOCUS_HOST", "compute-node")
    rd = remote_dir()
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
    red = ", ".join(f"{k} {v}" for k, v in sorted(r.redacted.items())) or "nothing"
    sent = "sent to the server" if r.sent else (f"NOT sent: {r.error}" if r.error else "not sent")
    skipped = ", ".join(f"{why} {n}" for why, n in r.attachments_skipped.most_common()) or "none"
    return (f"{r.kept} kept, {r.left_out} left out, {r.excluded} never leave the laptop; "
            f"attachments: {r.attachments_read} read, {r.attachments_empty} without text, skipped {skipped}; "
            f"{r.threads_written} threads written, {r.threads_removed} removed; redacted: {red}; {sent}")
