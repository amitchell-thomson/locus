"""Harvest arXiv metadata — the only part of Locus that reaches outward on a schedule.

WHAT LEAVES THIS MACHINE, exhaustively: a subject CATEGORY (`q-fin.PM`), a result offset, and a
page size. That is the entire outbound query. No concept, no project name, no gap term, no note
text, no title from the corpus.

That is a deliberate design choice and it is the reason this channel exists at all. The obvious
way to find relevant papers is to search for his topics — and that would ship his research
vocabulary to a third party on a timer. "market regime detection, walk-forward cross-validation,
Optibook" describes what he is BUILDING; a category code describes nothing about him. So the
relevance step happens entirely locally instead: harvest a broad slice by subject, embed the
abstracts on his own GPU, and rank against his own projects (`discover/rank.py`). The network sees
a librarian request; the judgement never leaves the room.

The constraint is enforced STRUCTURALLY, not by convention: `_CATEGORY` rejects anything that is
not an arXiv category token, and the query builder takes categories and nothing else. There is no
parameter through which corpus text could reach the URL, so a future caller cannot leak by
accident (principle 1).

arXiv asks unauthenticated clients to leave ~3s between requests; `harvest` sleeps between pages.
"""

from __future__ import annotations

import logging
import re
import time
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from typing import Callable, Iterable

log = logging.getLogger(__name__)

API = "http://export.arxiv.org/api/query"
_ATOM = "{http://www.w3.org/2005/Atom}"
_ARXIV = "{http://arxiv.org/schemas/atom}"

# An arXiv category token and NOTHING else: letters, one optional dot-suffix, hyphens allowed
# (`econ.EM`, `q-fin.PM`, `stat.ML`). This is the egress guard — see the module docstring.
_CATEGORY = re.compile(r"^[a-z][a-z-]{1,15}(\.[A-Za-z]{2,3})?$")

# Default slice: the quant/ML/econ surface his work actually sits on. Tunable in [discovery].
DEFAULT_CATEGORIES: tuple[str, ...] = (
    "q-fin.PM",   # portfolio management
    "q-fin.ST",   # statistical finance
    "q-fin.TR",   # trading & microstructure
    "q-fin.RM",   # risk management
    "q-fin.CP",   # computational finance
    "econ.EM",    # econometrics
    "stat.ML",    # machine learning (statistics)
    "stat.AP",    # applied statistics
    # State estimation and tracking, for tanker-flow: interpolating AIS gaps is a tracking
    # problem and no q-fin category carries the method. `eess.SP` was tried FIRST and removed —
    # measured 2026-07-31, its recent output is entirely wireless telecom (RIS, beamforming,
    # MIMO, GNSS, OTFS-NOMA): 19 papers harvested, none about tracking or filtering. These two
    # were then sampled before being trusted: ~4-5 of every 15 mention Kalman filtering, state
    # estimation or odometry.
    "eess.SY",    # systems & control — filtering, distributed state estimation
    "cs.RO",      # robotics — tracking, odometry, sensor fusion
)

# argv-free injection point: takes a URL, returns the response body. Tests pass a fake.
Fetcher = Callable[[str], str]


@dataclass(frozen=True)
class ArxivPaper:
    external_id: str        # 'arxiv:2607.12345'
    title: str
    authors: str
    abstract: str
    primary_category: str
    categories: str
    published: str
    url: str
    pdf_url: str


def _default_fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": "locus-discovery/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read().decode("utf-8", errors="replace")


def build_query(categories: Iterable[str], *, start: int = 0, page: int = 100) -> str:
    """The request URL. Accepts CATEGORIES ONLY — there is no free-text parameter by design.

    Raises on anything that is not an arXiv category token, so corpus vocabulary cannot reach the
    wire even if a future caller passes it by mistake.
    """
    cats = list(categories)
    if not cats:
        raise ValueError("at least one arXiv category is required")
    for c in cats:
        if not _CATEGORY.match(c):
            raise ValueError(
                f"{c!r} is not an arXiv category. Only category tokens may be sent outbound "
                "(locus/discover/arxiv.py explains why)."
            )
    params = {
        "search_query": " OR ".join(f"cat:{c}" for c in cats),
        "start": str(start),
        "max_results": str(page),
        "sortBy": "submittedDate",
        "sortOrder": "descending",
    }
    return f"{API}?{urllib.parse.urlencode(params)}"


def _text(node, tag: str) -> str:
    found = node.find(tag)
    return " ".join((found.text or "").split()) if found is not None else ""


def parse(xml: str) -> list[ArxivPaper]:
    """Atom feed -> papers. A malformed entry is skipped, never guessed at."""
    try:
        root = ET.fromstring(xml)
    except ET.ParseError as exc:
        raise RuntimeError(f"arXiv returned unparseable XML: {exc}") from exc

    out: list[ArxivPaper] = []
    for entry in root.findall(f"{_ATOM}entry"):
        raw_id = _text(entry, f"{_ATOM}id")
        title = _text(entry, f"{_ATOM}title")
        abstract = _text(entry, f"{_ATOM}summary")
        if not raw_id or not title or not abstract:
            continue  # nothing to rank on

        # 'http://arxiv.org/abs/2607.12345v1' -> '2607.12345'
        stem = raw_id.rsplit("/", 1)[-1]
        arxiv_id = re.sub(r"v\d+$", "", stem)

        authors = ", ".join(
            _text(a, f"{_ATOM}name") for a in entry.findall(f"{_ATOM}author")
        )
        cats = [
            c.attrib.get("term", "") for c in entry.findall(f"{_ATOM}category")
        ]
        primary = entry.find(f"{_ARXIV}primary_category")
        pdf = next(
            (l.attrib.get("href", "") for l in entry.findall(f"{_ATOM}link")
             if l.attrib.get("title") == "pdf"),
            f"http://arxiv.org/pdf/{arxiv_id}",
        )
        out.append(ArxivPaper(
            external_id=f"arxiv:{arxiv_id}",
            title=title,
            authors=authors,
            abstract=abstract,
            primary_category=(primary.attrib.get("term", "") if primary is not None
                              else (cats[0] if cats else "")),
            categories=" ".join(c for c in cats if c),
            published=_text(entry, f"{_ATOM}published")[:10],
            url=f"https://arxiv.org/abs/{arxiv_id}",
            pdf_url=pdf,
        ))
    return out


def harvest(
    categories: Iterable[str] = DEFAULT_CATEGORIES,
    *,
    per_category: int = 25,
    limit: int = 400,
    fetch: Fetcher | None = None,
    pause_s: float = 3.0,
) -> list[ArxivPaper]:
    """Fetch the most recent `per_category` papers from EACH category, newest first.

    ONE QUERY PER CATEGORY, NOT ONE OR-QUERY — and this is the difference between a useful pool
    and a useless one. arXiv sorts the combined result by date, and `stat.ML` and `cs.LG` publish
    roughly an order of magnitude more than the q-fin categories do. Measured on the first live
    harvest: a single OR-query for 200 papers returned 46 stat.ML, 31 cs.LG, 25 stat.ME... and
    exactly **2** q-fin.PM. 81% of the pool was general statistics and machine learning, so the
    ranking was picking the best of the wrong candidates and no amount of reweighting could have
    fixed it. Querying `q-fin.PM` on its own returns twenty portfolio-management papers at once.

    A per-category quota makes the pool's composition a decision rather than an accident of
    publication volume.
    """
    fetch = fetch or _default_fetch
    papers: list[ArxivPaper] = []
    seen: set[str] = set()

    for i, category in enumerate(categories):
        if len(papers) >= limit:
            break
        if i and pause_s:
            time.sleep(pause_s)  # arXiv asks unauthenticated clients to space requests
        url = build_query([category], start=0, page=per_category)
        for p in parse(fetch(url)):
            if p.external_id not in seen:
                seen.add(p.external_id)
                papers.append(p)
    return papers[:limit]
