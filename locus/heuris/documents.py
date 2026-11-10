"""Text out of attachments and career files, on the laptop, before anything leaves it (H1.12).

Found while the test set was corrected (10 Nov): the Millennium role preference form (Y2) is a
PDF he marked up and returned by mail, and the Millennium contract (24) is a file in
`~/Downloads`. The index held attachment names only, so neither could be answered.

**What is never read, by name** (`denied`). His identity and admin papers travel in the same
mail and folders as the offers: passports and a driving licence in `Career Documents/`, the P46
and the intern information form he sent Brevan Howard on 3 Oct. They hold the numbers that
matter most to keep off a server index and answer none of the test questions, so they are
skipped before their bytes are opened, and counted, so the push report says what it skipped.
Someone else's CV (`Aaron Rose CV.pdf`, the OQTS CV book sent to Jump) is skipped the same way:
a CV is read only when its name is his. Bank mandate forms (JCR signatories) are skipped by name.
Invoices, battels and remittance slips are read: the payee bank details on them are redacted.

**What is redacted in whatever is read** (`scrub_identifiers`), after the secret scrubber: UK
national insurance numbers, passport machine-readable lines, IBANs, a sort code with an account
number, and card numbers that pass the Luhn check. The name rule is the first line; this is the
second, for the same numbers turning up inside an offer pack or a forwarded form.

A scanned page with no text layer yields nothing and is skipped: no OCR here, by design, since
the scans in these folders are the identity documents.

Text is cached by content hash (`documents-cache.json` in the state dir), so the 15-minute
push reads each file once, not every tick.
"""

from __future__ import annotations

import email
import email.policy
import hashlib
import io
import json
import re
from pathlib import Path

from locus.scrub import scrub

MAX_BYTES = 25 * 1024 * 1024
MAX_CHARS = 150_000
# Part of the cache key: bump it when extraction changes, or cached text outlives the reader.
EXTRACT_VERSION = 2  # 2: hand-marked checkboxes (10 Nov)
READABLE = (".pdf", ".docx", ".txt", ".md")

DENY = re.compile(
    r"passport|driv.{0,6}licen[cs]e|\bP46\b|starter checklist|information form|data collection form|"
    r"\bstatement\b|bank|payslip|pay slip|\bP60\b|\bvisa\b|right.to.work|\bBRP\b|residence permit|"
    r"birth cert|national insurance|\btax\b|HMRC|claim form|medical|\bID\b|mandate", re.I)
CV = re.compile(r"\bcvs?\b|cv.?book|resume|résumé", re.I)
MINE = re.compile(r"mitchell|\bAMT\b|\balec\b", re.I)


def denied(name: str) -> str | None:
    """Why a file is never read, or None. Judged on the name only, before opening it."""
    stem = Path(name).name
    if stem.startswith(("~$", ".")):
        return "a lock or hidden file"
    if not stem.lower().endswith(READABLE):
        return "not a readable kind"
    if DENY.search(stem):
        return "an identity, tax or bank document"
    if CV.search(stem) and not MINE.search(stem):
        return "someone else's CV"
    return None


# -- identifiers ------------------------------------------------------------------------------

_NI = re.compile(r"\b(?!BG|GB|NK|KN|TN|NT|ZZ)[A-CEGHJ-PR-TW-Z][A-CEGHJ-NPR-TW-Z] ?\d{2} ?\d{2} ?\d{2} ?[A-D]\b")
# A machine-readable line ends in `<` fillers, so it cannot end on a \b word boundary.
_MRZ = re.compile(r"(?<![A-Z0-9<])(?=[A-Z0-9<]*<<)[A-Z0-9<]{25,50}(?![A-Z0-9<])")
_IBAN = re.compile(r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){3,7}(?: ?[A-Z0-9]{1,4})?\b")
_SORT_ACCT = re.compile(r"(?i)(sort\s*code[^0-9\n]{0,20})\d{2}[- ]?\d{2}[- ]?\d{2}"
                        r"|(account\s*(?:no\.?|number)?[^0-9\n]{0,20})\d{8}\b")
_CARD = re.compile(r"\b(?:\d[ -]?){13,19}\b")
_PASSPORT_NO = re.compile(r"(?i)(passport\s*(?:no\.?|number)?[^A-Z0-9\n]{0,20})[A-Z0-9]{8,9}\b")


def _luhn(digits: str) -> bool:
    total, alt = 0, False
    for ch in reversed(digits):
        d = int(ch)
        if alt:
            d = d * 2 - 9 if d > 4 else d * 2
        total, alt = total + d, not alt
    return total % 10 == 0


def scrub_identifiers(text: str) -> tuple[str, dict[str, int]]:
    counts: dict[str, int] = {}

    def sub(kind: str, pat: re.Pattern[str], s: str, keep_group: bool = False) -> str:
        def rep(m: re.Match[str]) -> str:
            counts[kind] = counts.get(kind, 0) + 1
            label = next((g for g in m.groups() if g), "") if keep_group else ""
            return f"{label}[redacted:{kind}]"
        return pat.sub(rep, s)

    text = sub("mrz", _MRZ, text)
    text = sub("passport-no", _PASSPORT_NO, text, keep_group=True)
    text = sub("ni-number", _NI, text)
    text = sub("iban", _IBAN, text)
    text = sub("bank-account", _SORT_ACCT, text, keep_group=True)

    def card(m: re.Match[str]) -> str:
        digits = re.sub(r"\D", "", m.group(0))
        if 13 <= len(digits) <= 19 and _luhn(digits):
            counts["card"] = counts.get("card", 0) + 1
            return "[redacted:card]"
        return m.group(0)
    text = _CARD.sub(card, text)
    return text, counts


# -- extraction -------------------------------------------------------------------------------

def _marked_boxes(page) -> str:
    """The options he ticked on a form, which are drawings, not text.

    The Millennium questionnaire (Y2) was answered with crosses drawn over its checkboxes and
    flattened into the PDF: `get_text` returns every question and option and none of the answers.
    A checkbox is a small filled square; a mark is a stroke whose box covers at least half of it
    with its centre inside (the form's own 9pt glyph inside each box covers a quarter, and is not
    a mark). Each marked box is reported with the option text beside it and the question above.
    Pages with thousands of drawings (plots) are skipped: no form looks like that.
    """
    try:
        drawings = page.get_drawings()
    except Exception:
        return ""
    if not drawings or len(drawings) > 1500:
        return ""
    boxes = [d["rect"] for d in drawings if d.get("fill") is not None and d["type"] == "f"
             and 10 <= d["rect"].width <= 30 and abs(d["rect"].width - d["rect"].height) < 2]
    strokes = [d["rect"] for d in drawings if d["type"] == "s"]
    if not boxes or not strokes:
        return ""
    blocks = [(b[0], b[1], b[2], b[3], " ".join(b[4].split())) for b in page.get_text("blocks")]
    words = page.get_text("words")
    out = []
    for box in boxes:
        area = box.width * box.height
        hit = any(r.width * r.height >= 0.5 * area and box.contains((r.tl + r.br) / 2) and r != box
                  for r in strokes)
        if not hit:
            continue
        mid = (box.y0 + box.y1) / 2
        # Words, not blocks: a block can span both columns of options (Question 4, 10 Nov).
        left = max([bx.x1 for bx in boxes if bx.x1 < box.x0 and abs(bx.y0 - box.y0) < 40], default=0)
        beside = [w for w in words if left <= w[0] and w[2] <= box.x0 + 2 and abs((w[1] + w[3]) / 2 - mid) <= 14]
        option = " ".join(w[4] for w in sorted(beside, key=lambda w: (round(w[1]), w[0]))) or "(no text beside it)"
        above = [b for b in blocks if b[3] <= box.y0 and b[4].lower().startswith("question")]
        question = max(above, key=lambda b: b[3])[4] if above else ""
        out.append((box.y0, box.x0, f"- {question}: {option}" if question else f"- {option}"))
    if not out:
        return ""
    return "\n\nMarked by hand on this page (crosses or ticks drawn over the boxes):\n" + \
        "\n".join(t for _, _, t in sorted(out))


def extract(name: str, data: bytes) -> str:
    """Plain text of a PDF, Word or text file; empty when it has none (a scan) or cannot be read."""
    low = name.lower()
    try:
        if low.endswith(".pdf"):
            import pymupdf

            with pymupdf.open(stream=data, filetype="pdf") as doc:
                text = "\n\n".join(page.get_text() + _marked_boxes(page) for page in doc)
        elif low.endswith(".docx"):
            import docx

            d = docx.Document(io.BytesIO(data))
            parts = [p.text for p in d.paragraphs]
            for t in d.tables:
                for row in t.rows:
                    parts.append(" | ".join(c.text for c in row.cells))
            text = "\n".join(parts)
        elif low.endswith((".txt", ".md")):
            text = data.decode("utf-8", errors="replace")
        else:
            return ""
    except Exception:  # a damaged or encrypted file is skipped, never fatal to the push
        return ""
    text = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f]", "", text)
    text = re.sub(r"\n{3,}", "\n\n", text).strip()
    return text[:MAX_CHARS]


def clean(text: str, known: list[str]) -> tuple[str, dict[str, int]]:
    """Secrets, then identifiers."""
    text, c1 = scrub(text, known)
    text, c2 = scrub_identifiers(text)
    for k, v in c2.items():
        c1[k] = c1.get(k, 0) + v
    return text, c1


class Cache:
    """sha256 of the bytes -> cleaned text, kept between pushes."""

    def __init__(self, path: Path):
        self.path = path
        try:
            self.data: dict[str, str] = json.loads(path.read_text())
        except (OSError, ValueError):
            self.data = {}
        self.used: set[str] = set()

    def text(self, name: str, data: bytes, known: list[str], redacted: dict[str, int]) -> tuple[str, str]:
        sha = hashlib.sha256(data).hexdigest()
        key = f"{EXTRACT_VERSION}:{sha}"
        self.used.add(key)
        if key not in self.data:
            raw = extract(name, data)
            text, counts = clean(raw, known) if raw else ("", {})
            for k, v in counts.items():
                redacted[k] = redacted.get(k, 0) + v
            self.data[key] = text
        return sha, self.data[key]

    def save(self) -> None:
        keep = {k: v for k, v in self.data.items() if k in self.used}
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix(".part")
        tmp.write_text(json.dumps(keep, ensure_ascii=False))
        tmp.chmod(0o600)
        tmp.replace(self.path)


# -- Mail's own store -------------------------------------------------------------------------

def attachment_bytes(raw_ref: str | None, part_id, name: str) -> bytes | None:
    """An attachment's bytes from Apple Mail's store, never from the network.

    A `.partial.emlx` keeps its attachments beside it, in `Attachments/<message>/<part>/<name>`;
    a full `.emlx` carries them inside the MIME message after a byte-count line.
    """
    if not raw_ref:
        return None
    p = Path(raw_ref)
    msg_no = p.name.split(".")[0]
    adir = p.parent.parent / "Attachments" / msg_no
    if adir.is_dir():
        for cand in ([adir / str(part_id) / name] if part_id is not None else []) + sorted(adir.glob(f"*/{name}")):
            if cand.is_file() and cand.stat().st_size <= MAX_BYTES:
                return cand.read_bytes()
    if p.is_file() and not p.name.endswith(".partial.emlx") and p.stat().st_size <= MAX_BYTES * 2:
        raw = p.read_bytes()
        nl = raw.find(b"\n")
        try:
            msg = email.message_from_bytes(raw[nl + 1:], policy=email.policy.default)
        except Exception:
            return None
        for part in msg.walk():
            if part.get_filename() == name:
                data = part.get_payload(decode=True)
                return data if data and len(data) <= MAX_BYTES else None
    return None
