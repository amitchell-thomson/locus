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

    @property
    def why(self) -> str:
        """The grounded reason, rendered on the proposal. Never a bare 'you might like this'.

        States the two measurements and NOTHING beyond them. An earlier draft asserted "nothing in
        your corpus covers it" — which the live run printed next to a familiarity of 0.71 against a
        fit of 0.68, i.e. while the corpus demonstrably covered it BETTER than the project matched
        it. A reason that overstates its evidence is worse than a terse one: it is the same failure
        as an ungrounded citation, and the whole point of the why is that he can trust it.
        """
        anchor = ("your work on" if self.matched_kind == "project"
                  else "a concept your work uses but has never written up:")
        margin = self.fit - self.familiarity
        gloss = ("nothing you own is closer" if margin > 0
                 else "though you already hold related material")
        return (f"closest to {anchor} {self.matched_label} (fit {self.fit:.2f}; "
                f"nearest existing material {self.familiarity:.2f} — {gloss})")


def store(conn: sqlite3.Connection, papers) -> int:
    """Record harvested metadata. Idempotent by `external_id`; returns how many were new."""
    new = 0
    with conn:
        for p in papers:
            cur = conn.execute(
                "INSERT INTO discovery_candidates (external_id, dedupe_key, title, authors, "
                "abstract, primary_category, categories, published, url, pdf_url, source, "
                "harvested_at) VALUES (?,?,?,?,?,?,?,?,?,?,'arxiv',?) "
                "ON CONFLICT(external_id) DO NOTHING",
                (p.external_id, dedupe_key(p.title, p.authors), p.title, p.authors, p.abstract,
                 p.primary_category, p.categories, p.published, p.url, p.pdf_url, _utcnow()),
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
) -> list[Scored]:
    """Best candidates for him right now, most promising first.

    Walks PROFILES outward rather than candidates inward: there are ~20 profiles and potentially
    thousands of candidates, so one KNN per profile is far cheaper than one per candidate and
    returns the same shortlist.
    """
    profiles = conn.execute(
        "SELECT p.id, p.subject_kind, p.label, p.doc_ids, v.embedding FROM discovery_profiles p "
        "JOIN discovery_profile_vectors v ON v.profile_id = p.id"
    ).fetchall()
    if not profiles:
        log.warning("no discovery profiles — run the profile rebuild first")
        return []

    # candidate_id -> (weighted fit, kind, label, the profile's own source docs)
    best: dict[int, tuple[float, str, str, set[int]]] = {}
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
                    fit, prof["subject_kind"], prof["label"], own_docs
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
        fit, kind, label, own_docs = best[r["id"]]
        fam = _familiarity(conn, r["embedding"], own_docs)
        raw.append((r, fit, fam, kind, label))

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

    scored = [
        Scored(
            candidate_id=r["id"], external_id=r["external_id"], title=r["title"],
            authors=r["authors"] or "", url=r["url"] or "", pdf_url=r["pdf_url"] or "",
            abstract=r["abstract"], fit=fit, familiarity=fam,
            score=fit - familiarity_weight * fam, matched_kind=kind, matched_label=label,
        )
        for (r, fit, fam, kind, label) in gated
    ]

    scored.sort(key=lambda s: -s.score)
    return _cap_per_profile(scored, limit=limit)


def _cap_per_profile(scored: list[Scored], *, limit: int, per_profile: int = 2) -> list[Scored]:
    """At most `per_profile` candidates from any one project or gap.

    Without it a single broad-vocabulary profile monopolises the list — measured on the first live
    run, where one profile took 8 of 12 slots. The reading list is meant to span his work, not to
    report which profile happens to embed nearest the harvest.
    """
    out: list[Scored] = []
    seen: dict[str, int] = {}
    for s in scored:
        key = f"{s.matched_kind}:{s.matched_label}"
        if seen.get(key, 0) >= per_profile:
            continue
        seen[key] = seen.get(key, 0) + 1
        out.append(s)
        if len(out) >= limit:
            break
    return out
