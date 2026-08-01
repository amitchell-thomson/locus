"""Score harvested candidates against his work — locally, deterministically, no model.

THE SCORE, and the reasoning behind its shape:

    score = fit - familiarity_penalty

`fit` is the best cosine between a candidate's abstract and one of his profiles (a project, or an
open gap). It does the overwhelming majority of the work, and the 2026-07-31 validation is
unambiguous about that: judged against real output, fit alone produced the best list.

`familiarity` is the best cosine between the candidate and anything already in the corpus, and it
is SUBTRACTED — the intent being to reward work close to a project he is building but unlike
anything he owns, so that a Kalman-filtering paper from robotics can reach the AIS project
precisely because no shipping paper in his corpus resembles it.

THAT INTENT IS RIGHT AND THE TERM IS CURRENTLY WEAK, which is worth stating plainly rather than
implying the design is fully vindicated. Subtracting familiarity presumes the corpus is dense
enough that "he already has this" is often TRUE. His is 205 documents of which 14 are papers, so
the nearest existing material to a quant abstract is usually unrelated coursework, and the term
mostly measures noise. Swept at 1.0 it actively destroyed the ranking (see FAMILIARITY_WEIGHT).
It is kept at 0.25 as a tiebreaker and should be raised as density grows — the density this
feature exists to create.

Everything here is a cosine and an arithmetic comparison. A model could write a prettier `why`,
but the why is "closest to this project, and nothing you own resembles it" — already true without
one, and a model in this loop would add spend, latency and a hallucination surface to a pipeline
that needs none of the three.

sqlite-vec's vec0 ranks by L2 distance, and nomic returns unit-normalised vectors, so
`cos = 1 - d^2 / 2` is exact rather than an approximation.
"""

from __future__ import annotations

import json
import logging
import math
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timezone

from locus.discover.profiles import vec_blob
from locus.reading.proposals import dedupe_key

log = logging.getLogger(__name__)

# How much a gap match counts relative to a project match. Projects are what he is actually
# building; a gap is a concept he has not written up, which is weaker evidence of intent.
GAP_WEIGHT = 0.6
# How hard to punish "you already have this".
#
# SWEPT AGAINST REAL OUTPUT 2026-07-31 (0.0 / 0.25 / 0.5 / 1.0 over a 297-candidate pool), and
# taken 1.0 -> 0.25. At 1.0 the term was actively destructive: the two best papers in the whole
# harvest for the Alpha Fund project ("Portfolio Optimization and Tail-Risk Analytics of Actively
# Managed ETFs", fit 0.75, and "AlphaZeroBeta: Deep RL for Market-Neutral Portfolios") were pushed
# out of the top 10 entirely, and their slots went to building-energy measurement-and-verification
# and PDE identification. At 0.25 both lead the list.
#
# The premise was right but premature. Subtracting familiarity assumes the corpus is DENSE enough
# that "he already has this" is often true; his is 205 documents of which 14 are papers, so the
# nearest existing material is usually unrelated coursework and the term mostly measures noise.
# It is kept as a TIEBREAKER rather than deleted, and should rise as density does — which is the
# whole point of the feature.
FAMILIARITY_WEIGHT = 0.25
# Fraction of the fit-ranked shortlist that survives the relevance gate. Novelty only orders what
# is already relevant — see `rank` for the two live runs that established this.
GATE_FRACTION = 0.25
# Candidates pulled per profile before the familiarity pass. Brute-force KNN at personal scale.
PER_PROFILE_K = 25
# Corpus sections examined when measuring familiarity. Over-fetched because the profile's own
# source documents are filtered out afterwards (vec0 KNN cannot express the join condition).
FAMILIARITY_K = 40
# ms-marco-MiniLM budgets 512 tokens across the PAIR, so the facet half is capped to leave
# room for the abstract it is being compared against.
_CROSS_QUERY_CHARS = 900


def _utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


def cos_from_l2(distance: float) -> float:
    """Exact for unit-normalised vectors, which is what nomic returns."""
    return 1.0 - (distance * distance) / 2.0


@dataclass
class Scored:
    candidate_id: int
    external_id: str
    title: str
    authors: str
    url: str
    pdf_url: str
    abstract: str
    fit: float
    familiarity: float
    score: float
    matched_kind: str    # 'project' | 'gap'
    matched_label: str
    cross_score: float | None = None   # None when the reranker was unavailable
    found_term: str | None = None      # the concept whose search returned this paper
    found_kind: str | None = None      # 'reading' | 'project' | 'gap' | 'browse'
    found_label: str | None = None     # the document/project that concept came from
    cited_by: int | None = None        # None means UNKNOWN (arXiv), never zero
    venue: str | None = None

    @property
    def why(self) -> str:
        """The grounded reason, rendered on the proposal. Never a bare 'you might like this'.

        States the two measurements and NOTHING beyond them. An earlier draft asserted "nothing in
        your corpus covers it" — which the live run printed next to a familiarity of 0.71 against a
        fit of 0.68, i.e. while the corpus demonstrably covered it BETTER than the project matched
        it. A reason that overstates its evidence is worse than a terse one: it is the same failure
        as an ungrounded citation, and the whole point of the why is that he can trust it.
        """
        # A real query beats a similarity score. When the paper was found by searching a concept
        # he underlined, say so — that is a fact he can check, not a number he has to trust.
        if self.found_term and self.found_kind in ("marked", "reading", "project", "gap"):
            origin = {
                "marked": f"a passage you underlined in {self.found_label or 'your reading'}",
                "reading": f"a concept from {self.found_label or 'your reading'}, which you annotated",
                "project": f"a method your {self.found_label or 'project'} work uses",
                "gap": "a concept your work uses but has never written up",
            }[self.found_kind]
            return f'found by searching "{self.found_term}" — {origin}'
        anchor = ("your work on" if self.matched_kind == "project"
                  else "a concept your work uses but has never written up:")
        margin = self.fit - self.familiarity
        gloss = ("nothing you own is closer" if margin > 0
                 else "though you already hold related material")
        return (f"closest to {anchor} {self.matched_label} (fit {self.fit:.2f}; "
                f"nearest existing material {self.familiarity:.2f} — {gloss})")


def store(conn: sqlite3.Connection, papers, *, term=None) -> int:
    """Record harvested metadata. Idempotent by `external_id`; returns how many were new.

    `papers` is either a list of papers (a category browse) or a list of `(paper, term)` pairs
    from a concept search. The term is stored because it IS the reason the paper is here — see
    migration 0020.
    """
    new = 0
    with conn:
        for item in papers:
            p, t = item if isinstance(item, tuple) else (item, term)
            kind = getattr(t, "source_kind", None) or ("browse" if t is None else "search")
            with_term = getattr(t, "term", t if isinstance(t, str) else None)
            cur = conn.execute(
                "INSERT INTO discovery_candidates (external_id, dedupe_key, title, authors, "
                "abstract, primary_category, categories, published, url, pdf_url, source, "
                "harvested_at, found_term, found_kind, found_label, cited_by, venue, doi) "
                "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) "
                "ON CONFLICT(external_id) DO NOTHING",
                (p.external_id, dedupe_key(p.title, p.authors), p.title, p.authors, p.abstract,
                 getattr(p, "primary_category", None), getattr(p, "categories", None),
                 p.published, p.url, p.pdf_url,
                 "openalex" if p.external_id.startswith("openalex:") else "arxiv",
                 _utcnow(), with_term, kind, getattr(t, "source_label", None),
                 getattr(p, "cited_by", None), getattr(p, "venue", None), getattr(p, "doi", None)),
            )
            new += cur.rowcount
    return new


def embed_pending(conn: sqlite3.Connection, *, embed_fn=None, batch: int = 64) -> int:
    """Embed every candidate that has no vector yet. Returns how many were embedded."""
    if embed_fn is None:
        from locus.ingest.embed import embed_texts as embed_fn

    rows = conn.execute(
        "SELECT id, title, abstract FROM discovery_candidates WHERE embedded = 0"
    ).fetchall()
    if not rows:
        return 0

    done = 0
    for start in range(0, len(rows), batch):
        window = rows[start : start + batch]
        # Title AND abstract: the title carries the method name, which is often the transferable
        # part and is diluted if only the abstract is embedded.
        vectors = embed_fn([f"{r['title']}. {r['abstract']}" for r in window])
        with conn:
            for r, vec in zip(window, vectors):
                # vec0 virtual tables do not implement UPSERT, so a re-embed is delete+insert.
                conn.execute(
                    "DELETE FROM discovery_vectors WHERE candidate_id = ?", (r["id"],)
                )
                conn.execute(
                    "INSERT INTO discovery_vectors (candidate_id, embedding) VALUES (?,?)",
                    (r["id"], vec_blob(vec)),
                )
                conn.execute(
                    "UPDATE discovery_candidates SET embedded = 1 WHERE id = ?", (r["id"],)
                )
        done += len(window)
    return done


def _familiarity(
    conn: sqlite3.Connection, blob: bytes, exclude_doc_ids: set[int]
) -> float:
    """Best cosine to corpus material OTHER than the profile's own source documents.

    Section summaries are the right comparison surface: they describe what a document is ABOUT at
    roughly the granularity of an abstract. Chunks would match on shared boilerplate phrasing, and
    propositions are far narrower than a paper-level claim.

    THE EXCLUSION IS LOad-BEARING, and leaving it out silently breaks the whole ranking. A project
    profile is built from his own write-ups of that project, and those write-ups are in the corpus
    — so without the exclusion the nearest existing material to any candidate matching a project
    is always that same write-up, familiarity equals fit, and `fit - familiarity` collapses to
    zero for every candidate alike. The ranking would then be pure noise while looking perfectly
    reasonable. Excluding the sources turns it into the question worth asking: does anything
    OTHER than my own description of the problem already teach this?

    Over-fetches and filters because vec0 KNN cannot express the join condition itself.
    """
    rows = conn.execute(
        "SELECT s.doc_id AS doc_id, k.distance AS distance FROM "
        "(SELECT section_id, distance FROM section_vectors "
        " WHERE embedding MATCH ? ORDER BY distance LIMIT ?) k "
        "JOIN sections s ON s.id = k.section_id ORDER BY k.distance",
        (blob, FAMILIARITY_K),
    ).fetchall()
    for r in rows:
        if r["doc_id"] not in exclude_doc_ids:
            return cos_from_l2(r["distance"])
    return 0.0


def rank(
    conn: sqlite3.Connection,
    *,
    limit: int = 10,
    familiarity_weight: float = FAMILIARITY_WEIGHT,
    gap_weight: float = GAP_WEIGHT,
    gate_fraction: float = GATE_FRACTION,
    citation_weight: float = 0.0,
    judge: dict | None = None,
    judge_floor: int = 1,
) -> list[Scored]:
    """Best candidates for him right now, most promising first.

    Walks PROFILES outward rather than candidates inward: there are ~20 profiles and potentially
    thousands of candidates, so one KNN per profile is far cheaper than one per candidate and
    returns the same shortlist.
    """
    profiles = conn.execute(
        "SELECT p.id, p.subject_kind, p.label, p.doc_ids, p.text, v.embedding "
        "FROM discovery_profiles p JOIN discovery_profile_vectors v ON v.profile_id = p.id"
    ).fetchall()
    if not profiles:
        log.warning("no discovery profiles — run the profile rebuild first")
        return []

    # candidate_id -> (weighted fit, kind, label, the profile's own source docs, facet text)
    best: dict[int, tuple[float, str, str, set[int], str]] = {}
    for prof in profiles:
        weight = gap_weight if prof["subject_kind"] == "gap" else 1.0
        try:
            own_docs = set(json.loads(prof["doc_ids"] or "[]"))
        except (TypeError, ValueError):
            own_docs = set()
        for r in conn.execute(
            "SELECT candidate_id, distance FROM discovery_vectors "
            "WHERE embedding MATCH ? ORDER BY distance LIMIT ?",
            (prof["embedding"], PER_PROFILE_K),
        ):
            fit = cos_from_l2(r["distance"]) * weight
            current = best.get(r["candidate_id"])
            if current is None or fit > current[0]:
                best[r["candidate_id"]] = (
                    fit, prof["subject_kind"], prof["label"], own_docs, prof["text"]
                )

    if not best:
        return []

    marks = ",".join("?" * len(best))
    rows = conn.execute(
        f"SELECT c.*, v.embedding FROM discovery_candidates c "
        f"JOIN discovery_vectors v ON v.candidate_id = c.id WHERE c.id IN ({marks})",
        list(best),
    ).fetchall()

    raw: list[tuple] = []
    for r in rows:
        fit, kind, label, own_docs, facet_text = best[r["id"]]
        fam = _familiarity(conn, r["embedding"], own_docs)
        raw.append((r, fit, fam, kind, label, facet_text))

    # RELEVANCE GATES; NOVELTY SORTS. Both live runs argued for this shape by failing without it.
    #
    # Treating the two terms as co-equal (`fit - familiarity`) does not work in either scaling.
    # On raw cosines nomic packs everything into a 0.65-0.75 band, so the difference was noise in
    # the third decimal. Standardising both and subtracting was worse: novelty then dominated, and
    # the top of the list filled with papers unlike ANYTHING he owns because they were unlike
    # anything at all — satellite geolocalisation, religious-radio transcripts, gut microbiome.
    # Being unfamiliar is only a virtue in something already relevant.
    #
    # So fit is a FILTER, not a summand: keep the best-fitting `gate_fraction` of the shortlist,
    # then order what survives by how little of it he already has. `score` is reported on the
    # surviving set as `fit - w * familiarity`, which is interpretable because every candidate
    # there has already cleared the relevance bar.
    # The gate never cuts below the number of results being asked for: throwing away candidates
    # that would otherwise have been returned is not filtering, it is just truncation.
    raw.sort(key=lambda t: -t[1])
    gated = raw[: max(limit, int(len(raw) * gate_fraction))]

    # CROSS-ENCODER RERANK — the same second stage every corpus query already runs through.
    # Discovery was ranking on bi-encoder cosine alone, i.e. half the engine: `retrieve/rerank.py`
    # exists precisely because a bi-encoder's cosine is a coarse similarity and cannot tell
    # "this method applies to that problem" from "these two texts share vocabulary". Reusing it
    # here costs nothing (CPU, already a dependency) and is the single biggest available lever.
    #
    # A failure is non-fatal: without the optional `[rerank]` extra installed the cosine ordering
    # still stands, degraded rather than broken (the same graceful-degradation rule the rest of the
    # pipeline follows for optional system deps).
    cross = _cross_scores([(facet, f"{r['title']}. {r['abstract']}") for r, _, _, _, _, facet in gated])

    bonuses = _citation_bonuses([r["cited_by"] for r, _, _, _, _, _ in gated])
    scored = [
        Scored(
            candidate_id=r["id"], external_id=r["external_id"], title=r["title"],
            authors=r["authors"] or "", url=r["url"] or "", pdf_url=r["pdf_url"] or "",
            abstract=r["abstract"], fit=fit, familiarity=fam,
            score=((cross[i] if cross else fit)
                   - familiarity_weight * fam
                   + citation_weight * bonuses[i]),
            cross_score=cross[i] if cross else None,
            matched_kind=kind, matched_label=label,
            found_term=r["found_term"], found_kind=r["found_kind"],
            found_label=r["found_label"], cited_by=r["cited_by"], venue=r["venue"],
        )
        for i, (r, fit, fam, kind, label, _facet) in enumerate(gated)
    ]

    scored.sort(key=lambda s: -s.score)
    shortlist = _cap_per_profile(scored, limit=limit * 2 if judge else limit)
    if judge:
        shortlist = _apply_judge(conn, shortlist, judge=judge, drop_at_or_below=judge_floor)
    return shortlist[:limit]


def _citation_bonuses(counts: list) -> list[float]:
    """Log-citations CENTRED on the pool median, so the prior can only ever break ties.

    The first version returned raw `log10(1+citations)` and scored UNKNOWN as 0.0, on the
    reasoning that absence must not be read as "uncited". Measured, that reasoning was half
    right and the implementation was wrong: the median known count in the live pool is 601,
    worth +0.42 at weight 0.15, while arXiv reports no count at all and therefore scored 0.00.
    Every preprint sat 0.42 behind every median journal article for no reason but its source —
    exactly the systematic demotion the zero was meant to prevent.

    Centring fixes it. The bonus is the distance from the pool's median in log space, so a
    well-cited work earns a little, a barely-cited one loses a little, and an UNKNOWN count maps
    to precisely 0.0 — the middle of the field rather than the bottom of it.

    The magnitude is deliberately small and log10 keeps it that way: across the entire live pool
    the term spans about 0.76, against a cross-encoder interquartile range of 3.59. Citations
    order papers that are already relevant; they never promote one that is not.
    """
    known = [math.log10(1.0 + max(int(c), 0)) for c in counts if c is not None]
    if not known:
        return [0.0] * len(counts)
    known.sort()
    median = known[len(known) // 2]
    out: list[float] = []
    for c in counts:
        if c is None:
            out.append(0.0)                 # unknown == the median, i.e. no opinion
        else:
            out.append(math.log10(1.0 + max(int(c), 0)) - median)
    return out


def _apply_judge(conn, shortlist, *, judge, drop_at_or_below: int):
    """Drop clear irrelevance. Order is untouched — the judge has no resolution to rank with."""
    from locus.discover import judge as J

    facets = {}
    for s in shortlist:
        row = conn.execute(
            "SELECT text FROM discovery_profiles WHERE label = ? ORDER BY LENGTH(text) DESC "
            "LIMIT 1", (s.matched_label,),
        ).fetchone()
        facets[s.candidate_id] = row["text"] if row else s.matched_label

    verdicts = J.score(
        [(s.matched_label, facets[s.candidate_id], s.title, s.abstract) for s in shortlist],
        model=judge.get("model", ""), host=judge.get("host", ""),
        judge_fn=judge.get("judge_fn"),
    )
    kept = [s for s, v in zip(shortlist, verdicts) if v.score > drop_at_or_below]
    dropped = len(shortlist) - len(kept)
    if dropped:
        log.info("judge dropped %d candidate(s) scoring <= %d", dropped, drop_at_or_below)
    return kept


def _cross_scores(pairs: list[tuple[str, str]]) -> list[float]:
    """Cross-encoder relevance for (profile facet, candidate) pairs; [] if unavailable.

    The facet is truncated because ms-marco-MiniLM takes 512 tokens across BOTH halves of the
    pair — feeding it a 4000-character section summary would push the candidate's own abstract
    out of the window entirely, scoring the paper against nothing.
    """
    if not pairs:
        return []
    try:
        from locus.retrieve.rerank import _cross_encoder

        model = _cross_encoder()
        return [
            float(x) for x in model.predict([(q[:_CROSS_QUERY_CHARS], t) for q, t in pairs])
        ]
    except Exception as exc:                       # extra not installed, model not cached, OOM
        log.warning("cross-encoder unavailable (%s) — falling back to cosine ordering", exc)
        return []


def _cap_per_profile(scored: list[Scored], *, limit: int, per_profile: int = 2) -> list[Scored]:
    """Interleave by subject, best-first, so every project gets its best paper before any gets two.

    A flat global sort is unfair in a way that is easy to miss, because the numbers look
    comparable and are not. Measured 2026-07-31: tanker-flow's best facet scores 0.61 while Alpha
    Fund's scores 0.76 — a difference in how EMBEDDABLE the two descriptions are, not in how
    relevant the papers are. Sorting globally hands Alpha Fund the slots permanently and tanker-
    flow never appears, however good its top candidate is for it.

    Round-robin fixes that: one pass gives each subject its single best candidate, the next pass
    gives seconds, and only then does rank order decide. The earlier version capped each subject
    at two but still filled the list in global order, which let the loud subjects take the whole
    first page anyway.
    """
    # THE SUBJECT IS THE CONCEPT THAT FOUND IT, when one did. A paper returned by searching
    # "liquidity-aware portfolio optimization" has already demonstrated relevance to a term he
    # underlined by hand; making it re-compete on cosine similarity to a project blob discards
    # that evidence and buries it. Measured before this change: 79 search hits produced exactly
    # 2 of the top 12. Grouping by the search term instead gives every concept he cares about its
    # own best paper.
    # `match:` namespaces the candidates that no search found, so a browsed paper that happens to
    # sit closest to a PROJECT profile is not bucketed as though a project search had returned it.
    by_subject: dict[str, list[Scored]] = {}
    for s in scored:
        key = (f"{s.found_kind}:{s.found_term}" if s.found_term
               else f"match:{s.matched_kind}:{s.matched_label}")
        by_subject.setdefault(key, []).append(s)

    # INTERLEAVE THE CHANNELS rather than ranking them. Strict priority looked right and was not:
    # his reading currently supplies 24 search terms against ~19 from projects, so ordering
    # reading-first handed it every slot and no project appeared at all. A reading list that only
    # reflects the last book he opened is as narrow as one that only reflects his code.
    channels: dict[str, list[str]] = {
        "marked": [], "reading": [], "project": [], "gap": [], "match": [],
    }
    for key in sorted(by_subject, key=lambda k: -by_subject[k][0].score):
        channels.setdefault(key.split(":", 1)[0], []).append(key)

    order: list[str] = []
    queues = [q for q in (channels["marked"], channels["reading"], channels["project"],
                          channels["gap"], channels["match"]) if q]
    for i in range(max((len(q) for q in queues), default=0)):
        for q in queues:
            if i < len(q):
                order.append(q[i])

    out: list[Scored] = []
    for round_ in range(per_profile):
        for key in order:
            if len(out) >= limit:
                return out
            if len(by_subject[key]) > round_:
                out.append(by_subject[key][round_])
    return out[:limit]
