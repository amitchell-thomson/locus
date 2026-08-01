"""Step 3 — mine the citations of what he already keeps.

The search channels ask "what has been written about the concepts he cares about". This one asks
a different and complementary question: **what do the works he already chose point at?** A paper
he kept, and a book he read with a pen in his hand, are both filtered recommendations already —
their bibliographies are a reading list assembled by someone who knew the subject.

CO-CITATION IS THE REAL SIGNAL, not citation. A work cited by ONE of his documents is a
suggestion; a work cited by TWO OR MORE is a cluster forming around what he actually reads, and
it is the closest thing available to consensus among sources he has personally endorsed. The two
are stored as distinct channels (`citation` / `co_citation`) so the flywheel can learn whether
that distinction pays.

WHY OPENALEX RATHER THAN PARSING THE PDFs. Measured 2026-07-31: 12 of his 13 papers carry a
`References` header, but a two-pattern parse recovered entries from only 6 of them — bibliography
formats vary enough that a robust parser is its own project with a long tail. `referenced_works`
returns the same edges exactly, already resolved to identifiers, for one request per document.

THIS CHANNEL RETURNS NOTHING TODAY, AND THAT IS A PROPERTY OF HIS CORPUS RATHER THAN A BUG.
Measured 2026-08-01 against the live API:

    his arXiv preprint (2605.30363)      type=preprint  referenced_works = 0
    a finance journal article (nbh004)   type=article   referenced_works = 31
    Sharpe 1964                          type=article   referenced_works = 0

**OpenAlex holds no reference lists for arXiv preprints**, and all eleven identifiable documents
in his corpus are arXiv preprints. Journal articles do carry them, and older classics sometimes do
not. The book is not indexed under a searchable title at all.

So the mechanism is built, correct and idle: it becomes useful the moment he accepts a JOURNAL
article — which the search channels now propose regularly (Christoffersen's VaR paper, with its 31
references, is sitting in `Proposed` as this is written). It is left switched on rather than
deferred because its cost when empty is eleven cheap requests, and its value arrives silently the
first time a citable document enters the corpus. A PDF bibliography parser remains the fallback
for books and preprints, and is now the only route to them.

Every outbound request carries a public identifier of a document he owns and nothing else.
"""

from __future__ import annotations

import json
import logging
import re
import sqlite3
import time
import urllib.parse
from collections import defaultdict
from dataclasses import dataclass
from typing import Callable, Iterable

log = logging.getLogger(__name__)

API = "https://api.openalex.org/works"
Fetcher = Callable[[str], str]

_ARXIV_ID = re.compile(r"(\d{4}\.\d{4,5})")
# OpenAlex accepts a pipe-separated id filter; 50 is their documented ceiling per request.
_BATCH = 50


@dataclass(frozen=True)
class CitedWork:
    """A work referenced by one or more of his documents."""

    work_id: str                  # 'W2741809807'
    citing_titles: tuple[str, ...]

    @property
    def channel(self) -> str:
        return "co_citation" if len(self.citing_titles) > 1 else "citation"


def corpus_identifiers(conn: sqlite3.Connection) -> list[tuple[str, str]]:
    """`(openalex_selector, document title)` for every corpus document we can look up.

    arXiv ids come free from the `source_uri` the file was ingested under; a DOI would be used the
    same way if one were stored. Documents with neither are skipped rather than guessed at — a
    title search would silently attribute someone else's bibliography to his reading.
    """
    out: list[tuple[str, str]] = []
    for r in conn.execute(
        "SELECT title, source_uri FROM documents "
        "WHERE source_type = 'pdf' AND category IN ('paper', 'project', 'note')"
    ):
        m = _ARXIV_ID.search(str(r["source_uri"] or ""))
        if m:
            # arXiv's DataCite DOI is the selector OpenAlex actually resolves. `arxiv:<id>` and
            # the abs URL both 404 — verified 2026-08-01 against a known-indexed paper, so this
            # is the format rather than a coverage gap.
            out.append((f"doi:10.48550/arXiv.{m.group(1)}", r["title"] or m.group(1)))
    return out


def _get(fetch: Fetcher, url: str) -> dict:
    from locus.discover.openalex import redact

    try:
        return json.loads(fetch(url))
    except (json.JSONDecodeError, OSError) as exc:
        # Redacted: the key rides in the query string and HTTPError stringifies the whole URL.
        raise RuntimeError(redact(f"OpenAlex request failed: {exc}")) from None


def referenced_works(
    identifiers: Iterable[tuple[str, str]], *, fetch: Fetcher, mailto: str = "",
    pause_s: float = 1.0,
) -> list[CitedWork]:
    """Every work his documents reference, with the documents that reference it.

    A failed lookup for one document is logged and skipped: a bibliography we could not read is a
    gap in coverage, never a reason to abandon the ones we could.
    """
    citing: dict[str, list[str]] = defaultdict(list)
    from locus.discover.openalex import api_key

    params = {k: v for k, v in (("mailto", mailto), ("api_key", api_key())) if v}
    suffix = f"?{urllib.parse.urlencode(params)}" if params else ""

    for i, (selector, title) in enumerate(identifiers):
        if i and pause_s:
            # Paced rather than bursted. Firing all eleven lookups at once is what earned a
            # sustained 429 on 2026-08-01, which the per-request backoff could not clear because
            # the cooldown outlasted it.
            time.sleep(pause_s)
        try:
            work = _get(fetch, f"{API}/{selector}{suffix}")
        except RuntimeError as exc:
            log.warning("no reference list for %s: %s", selector, exc)
            continue
        for ref in work.get("referenced_works") or ():
            wid = str(ref).rsplit("/", 1)[-1]
            if wid and title not in citing[wid]:
                citing[wid].append(title)

    if not citing:
        # Worth saying out loud rather than returning an empty list silently: "no citations found"
        # and "none of these documents HAS a reference list" are very different diagnoses, and
        # only the second is true today (see the module docstring).
        log.info(
            "no reference lists available for any of %d document(s) — OpenAlex holds none for "
            "arXiv preprints; this channel activates when a journal article is accepted",
            len(list(identifiers)) if isinstance(identifiers, list) else 0,
        )
    return [CitedWork(wid, tuple(titles)) for wid, titles in citing.items()]


def resolve(
    work_ids: Iterable[str], *, fetch: Fetcher, mailto: str = "", batch: int = _BATCH
) -> "list":
    """Batch-fetch metadata for referenced works. Returns `OpenAlexWork`s, reusing that parser."""
    from locus.discover.openalex import parse

    ids = [w for w in work_ids if w]
    out: list = []
    for start in range(0, len(ids), batch):
        window = ids[start : start + batch]
        params = {
            "filter": f"openalex_id:{'|'.join(window)}",
            "per_page": str(len(window)),
        }
        from locus.discover.openalex import api_key as _ak

        if mailto:
            params["mailto"] = mailto
        if _ak():
            params["api_key"] = _ak()
        try:
            out.extend(parse(fetch(f"{API}?{urllib.parse.urlencode(params)}")))
        except (RuntimeError, OSError) as exc:
            log.warning("could not resolve a batch of %d referenced works: %s", len(window), exc)
    return out


def harvest(
    conn: sqlite3.Connection,
    *,
    fetch: Fetcher | None = None,
    mailto: str = "",
    min_citing: int = 1,
    limit: int = 300,
) -> "list[tuple[object, object]]":
    """`(work, term)` pairs for storage, exactly the shape the search channels produce.

    `min_citing=2` restricts to co-citations — works two or more of his documents point at.
    """
    from locus.discover.openalex import _default_fetch
    from locus.discover.queries import SearchTerm

    fetch = fetch or _default_fetch
    identifiers = corpus_identifiers(conn)
    if not identifiers:
        return []

    cited = [c for c in referenced_works(identifiers, fetch=fetch, mailto=mailto)
             if len(c.citing_titles) >= min_citing]
    # Most-cited-by-him first, so a truncated budget spends itself on the strongest signal.
    cited.sort(key=lambda c: -len(c.citing_titles))
    cited = cited[:limit]
    if not cited:
        return []

    by_id = {c.work_id: c for c in cited}
    works = resolve(by_id, fetch=fetch, mailto=mailto)

    pairs: list[tuple[object, object]] = []
    for w in works:
        c = by_id.get(w.external_id.split(":", 1)[-1])
        if c is None:
            continue
        label = (f"{len(c.citing_titles)} papers you keep" if c.channel == "co_citation"
                 else c.citing_titles[0])
        pairs.append((w, SearchTerm(w.title[:60], c.channel, label)))
    return pairs
