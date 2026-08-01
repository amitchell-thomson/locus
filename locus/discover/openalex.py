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
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Callable, Iterable

log = logging.getLogger(__name__)

API = "https://api.openalex.org/works"
Fetcher = Callable[[str], str]

# Premium key, read from the environment ONLY — never config.toml, which is a settings file the
# owner edits and shares the shape of. Same contract as ANTHROPIC_API_KEY (config.py docstring).
_KEY_ENV = "OPENALEX_API_KEY"


def api_key() -> str:
    import os

    # Importing config is what populates os.environ from the project .env (config._load_dotenv
    # runs at import). Without it the key is invisible to any caller that reached this module
    # without touching config first — which is most of them.
    import locus.config  # noqa: F401

    return os.environ.get(_KEY_ENV, "").strip()


def redact(text: str) -> str:
    """Strip the key from anything that might be logged or raised.

    `HTTPError` stringifies with the FULL URL, and the key travels as a query parameter, so an
    unredacted warning would write the credential into the journal on every failed request —
    and failures are exactly when logging happens.
    """
    key = api_key()
    return text.replace(key, "***") if key else text

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


def _default_fetch(url: str, *, attempts: int = 4) -> str:
    """GET with backoff on 429.

    Hit for real on 2026-08-01: a single harvest makes one request per search term (73 of them),
    and the unauthenticated common pool starts returning `429 Too Many Requests` well inside that.
    Without a retry the weekly job would quietly harvest a fraction of the literature and report
    success, which is the worst failure shape available — silently less coverage, no error.

    Setting `[discovery].openalex_mailto` moves the client into OpenAlex's polite pool, where the
    limits are far higher; this backoff is what makes the run survive without it.
    """
    import time

    delay = 2.0
    for attempt in range(attempts):
        req = urllib.request.Request(url, headers={"User-Agent": "locus-discovery/1.0"})
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                return resp.read().decode("utf-8", errors="replace")
        except urllib.error.HTTPError as exc:
            if exc.code != 429 or attempt == attempts - 1:
                raise RuntimeError(redact(f"OpenAlex HTTP {exc.code}: {exc.url}")) from None
            log.info("OpenAlex rate-limited; backing off %.0fs", delay)
            time.sleep(delay)
            delay *= 2
    raise RuntimeError("unreachable")


def build_query(term: str, *, per_page: int = 25, mailto: str = "", exact: bool = True) -> str:
    """A relevance-ranked search of TITLES AND ABSTRACTS for one technical phrase.

    Also filtered to works that HAVE an abstract, since a work without one cannot be embedded,
    reranked or judged — it would occupy a slot on the strength of its title alone.
    """
    phrase = " ".join((term or "").split())
    if len(phrase) < 4:
        raise ValueError(f"search term too short: {term!r}")
    # TITLE AND ABSTRACT, not the general `search` parameter, which also matches FULLTEXT and is
    # far too loose to be useful. Measured 2026-08-01 on `Marginal Contribution to Factor Risk`:
    # `search=` returned 719,908 matches led by the 2019 ESC/EAS dyslipidaemia guidelines and a
    # global burden-of-disease study, because the words "risk", "factor" and "contribution" occur
    # all over medicine. The same phrase against title_and_abstract returns 1,879 — a 380x
    # narrowing — led by Sharpe's CAPM, which is the actual canonical answer.
    # QUOTED IS THE PRECISION TIER. Unquoted, OpenAlex ANDs the words anywhere in title or
    # abstract, which for a two-word concept is barely a filter at all. Measured 2026-08-01:
    #
    #   "Alternative Data"   768,995 -> 4,742 matches  (led by psychometrics and a genomics tool
    #                                                   -> by alternative data in FINANCE)
    #   "Information Ratio"  385,741 -> 1,071 matches  (led by Shannon's Elements of Information
    #                                                   Theory -> by "The Information Ratio")
    #
    # The caller falls back to unquoted when the phrase finds too little, so precision is tried
    # first and recall is the safety net — the same shape as the arXiv channel.
    quoted = f'"{phrase}"' if exact else phrase
    params = {
        "per_page": str(per_page),
        "filter": f"has_abstract:true,title_and_abstract.search:{quoted}",
    }
    if mailto:
        params["mailto"] = mailto      # the polite pool: better limits, and they ask for it
    if api_key():
        params["api_key"] = api_key()  # premium pool: far higher limits again
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
        if i and pause_s:
            import time

            time.sleep(pause_s)
        works = []
        # Precision, then recall: an exact phrase that finds nothing means nobody phrased the
        # concept his way, not that nobody works on it.
        for exact in (True, False):
            try:
                url = build_query(text, per_page=per_term, mailto=mailto, exact=exact)
            except ValueError:
                break
            try:
                works = parse(fetch(url))
            except (RuntimeError, OSError) as exc:
                log.warning("OpenAlex search for %r failed: %s", text, redact(str(exc)))
                works = []
            if works:
                break
        if not works:
            continue
        for w in works:
            if w.external_id not in seen:
                seen.add(w.external_id)
                out.append((w, term))
    return out[:limit]
