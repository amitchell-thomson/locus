"""Search OpenAlex — journals, books and chapters, which arXiv does not cover.

arXiv is a preprint server skewed to CS, physics and maths. Much of what he reads is not on it:
the portfolio-management canon lives in journals and books, and a search there returns nothing at
all. Measured 2026-07-31, an arXiv search for `Kalman filter AND trajectory interpolation` — a
problem he has explicitly written down — returned ZERO results, not because nobody works on it
but because that work is published elsewhere.

OpenAlex indexes ~250M works including journal articles, books and chapters, needs no API key,
and gives two things arXiv cannot: a citation count, which is the cheapest usable proxy for "this
is the canonical treatment rather than the newest variant", and an open-access URL when one
exists, which is what lets a proposal BE the paper instead of describing it.

WHAT LEAVES: the same short technical phrase the arXiv channel sends, plus a contact email in the
`mailto` parameter — OpenAlex's polite-pool convention, which buys higher rate limits and is how
they ask to be identified. Nothing about him, his projects or his notes.

ABSTRACTS ARRIVE INVERTED. OpenAlex stores `abstract_inverted_index` — `{word: [positions]}` —
rather than the abstract text, for copyright reasons. Reconstructing it is a scatter into a list
by position; that reconstruction is lossy on punctuation and that is fine, because the text is
only ever embedded and reranked, never shown as though it were the publisher's abstract.
"""

from __future__ import annotations

import json
import logging
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Callable, Iterable

log = logging.getLogger(__name__)

API = "https://api.openalex.org/works"
Fetcher = Callable[[str], str]

# Works with fewer characters of reconstructed abstract than this cannot be ranked meaningfully.
_MIN_ABSTRACT = 120


@dataclass(frozen=True)
class OpenAlexWork:
    external_id: str          # 'openalex:W2741809807'
    title: str
    authors: str
    abstract: str
    published: str            # ISO date or year
    url: str
    pdf_url: str              # '' when not open access
    venue: str
    cited_by: int
    doi: str


def _default_fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "locus-discovery/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read().decode("utf-8", errors="replace")


def build_query(term: str, *, per_page: int = 25, mailto: str = "") -> str:
    """A relevance-ranked search for one technical phrase.

    `search` covers title, abstract and fulltext. Filtered to works that HAVE an abstract, since a
    work without one cannot be embedded, reranked or judged — it would occupy a slot on the
    strength of its title alone.
    """
    phrase = " ".join((term or "").split())
    if len(phrase) < 4:
        raise ValueError(f"search term too short: {term!r}")
    params = {
        "search": phrase,
        "per_page": str(per_page),
        "filter": "has_abstract:true",
    }
    if mailto:
        params["mailto"] = mailto      # the polite pool: better limits, and they ask for it
    return f"{API}?{urllib.parse.urlencode(params)}"


def reconstruct_abstract(inverted: dict | None) -> str:
    """`{word: [positions]}` -> text. Lossy on punctuation, which does not matter here."""
    if not inverted:
        return ""
    slots: dict[int, str] = {}
    for word, positions in inverted.items():
        for pos in positions or ():
            slots[pos] = word
    if not slots:
        return ""
    return " ".join(slots[i] for i in sorted(slots))


def parse(payload: str) -> list[OpenAlexWork]:
    """OpenAlex JSON -> works. A record without a usable abstract is skipped, never guessed at."""
    try:
        data = json.loads(payload)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"OpenAlex returned unparseable JSON: {exc}") from exc

    out: list[OpenAlexWork] = []
    for w in data.get("results") or []:
        abstract = reconstruct_abstract(w.get("abstract_inverted_index"))
        title = (w.get("display_name") or "").strip()
        if not title or len(abstract) < _MIN_ABSTRACT:
            continue

        oid = (w.get("id") or "").rsplit("/", 1)[-1]
        if not oid:
            continue
        loc = w.get("primary_location") or {}
        source = loc.get("source") or {}
        oa = w.get("open_access") or {}
        authors = ", ".join(
            ((a.get("author") or {}).get("display_name") or "")
            for a in (w.get("authorships") or [])[:8]
        ).strip(", ")

        out.append(OpenAlexWork(
            external_id=f"openalex:{oid}",
            title=title,
            authors=authors,
            abstract=abstract,
            published=str(w.get("publication_date") or w.get("publication_year") or ""),
            url=w.get("doi") or (w.get("id") or ""),
            pdf_url=oa.get("oa_url") or "",
            venue=(source.get("display_name") or "").strip(),
            cited_by=int(w.get("cited_by_count") or 0),
            doi=(w.get("doi") or "").replace("https://doi.org/", ""),
        ))
    return out


def search(
    terms: Iterable,
    *,
    per_term: int = 10,
    limit: int = 400,
    mailto: str = "",
    fetch: Fetcher | None = None,
    pause_s: float = 1.0,
) -> "list[tuple[OpenAlexWork, object]]":
    """Search each term; returns `(work, term)` so the reason survives to the proposal."""
    fetch = fetch or _default_fetch
    out: list[tuple[OpenAlexWork, object]] = []
    seen: set[str] = set()

    for i, term in enumerate(terms):
        if len(out) >= limit:
            break
        text = getattr(term, "term", term)
        try:
            url = build_query(text, per_page=per_term, mailto=mailto)
        except ValueError:
            continue
        if i and pause_s:
            import time

            time.sleep(pause_s)
        try:
            works = parse(fetch(url))
        except (RuntimeError, OSError) as exc:
            log.warning("OpenAlex search for %r failed: %s", text, exc)
            continue
        for w in works:
            if w.external_id not in seen:
                seen.add(w.external_id)
                out.append((w, term))
    return out[:limit]
