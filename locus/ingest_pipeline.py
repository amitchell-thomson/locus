"""Ingest orchestration: one source file -> a fully populated 3-level DB (CLAUDE.md §6).

Per file (pdf / docx / markdown / text / notebook / slides, routed by suffix):
  hash -> skip if the content_hash already exists (idempotent)
       -> copy the raw file into the flat raw store
       -> extract (per-format, extract/) -> ordered sections
       -> per section: summary + propositions + entities + chunks  (local LLM + embeddings)
       -> doc: synthesis (thesis/method/result/limitations) + gap flags
       -> write L1 + L2 + L3 + entities in ONE transaction (atomic; rolls back on error)

Any failure quarantines that single file (logged, returned in the result) without aborting a
batch and without leaving a partial document (the write is transactional, and all expensive
work happens before the write opens).

Deferred (noted in §6): cross_section_deps and tags are not yet produced, so those columns
are written empty / left unpopulated.
"""

from __future__ import annotations

import hashlib
import json
import logging
import re
import shutil
import struct
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from locus.config import load
from locus.db.connection import get_connection
from locus.extract import code as code_extract
from locus.extract import docx as docx_extract
from locus.extract import pdf as pdf_extract
from locus.extract import pptx as pptx_extract
from locus.extract import textdoc
from locus.extract.base import ExtractedDoc, ExtractedSection, PreChunk
from locus.ingest import chunk, embed, entities, gaps, llm, propositions, summarize, synthesis
from locus.ingest import figures as figures_pass

log = logging.getLogger(__name__)


@dataclass
class IngestResult:
    path: str
    status: str  # "ingested" | "skipped" | "quarantined" | "unsupported"
    doc_id: int | None = None
    sections: int = 0
    chunks: int = 0
    propositions: int = 0
    entities: int = 0
    figures: int = 0
    error: str | None = None


# --- small helpers -----------------------------------------------------------------------


def _hash_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


# Suffix -> documents.source_type (the FORMAT axis, §15.4). code/video are routed by
# different entry points (a repo directory, a URL), not by file suffix here.
_SUFFIX_TYPE = {
    ".pdf": "pdf",
    ".docx": "docx",
    ".md": "markdown",
    ".markdown": "markdown",
    ".txt": "text",
    ".ipynb": "notebook",
    ".pptx": "slides",
}


def _source_type(path: Path) -> str | None:
    return _SUFFIX_TYPE.get(path.suffix.lower())


# Canonical kind names (CLAUDE.md §15.4/§16): folder names matching one of these (singular
# or plural) are normalized to it, so the core vocabulary stays stable across drops.
# One axis only — categories are KINDS of content; FORMAT lives in documents.source_type
# (so no 'code'/'slides' category: a repo under projects/ is category='project',
# source_type='code'). Authorship splits notes (owner-written) from coursework (handed to
# the owner by a course); career absorbs CV/applications/achievements (2026-06-05 taxonomy).
CATEGORIES = {"paper", "coursework", "project", "career", "note", "video"}


def _category(path: Path) -> str:
    """Derive a category from the drop location. The owner's folder taxonomy is authoritative.

    Primary rule: the first folder under vault/incoming/ IS the category, free-form —
    `incoming/engineering/x.pdf` → 'engineering', `incoming/papers/y.pdf` → 'paper' (known
    kinds are singularized so the §15.4 vocabulary stays canonical). The laptop outbox rsyncs
    subfolders verbatim, so sorting once in the local drop folder carries all the way through.
    Files ingested from outside incoming/ fall back to scanning path parts for a known kind;
    anything else is 'uncategorized'.
    """
    try:
        rel = path.resolve().relative_to(load().paths.incoming)
        if len(rel.parts) > 1:  # at least one folder between incoming/ and the file
            top = rel.parts[0].lower()
            norm = top.rstrip("s")  # papers -> paper, notes -> note
            return norm if norm in CATEGORIES else top
    except ValueError:
        pass  # not under incoming/ — use the known-kind scan below
    for part in path.parts:
        norm = part.lower().rstrip("s")
        if norm in CATEGORIES:
            return norm
    return "uncategorized"


def _mtime_date(path: Path) -> str:
    """File modification time as an ISO date — the fallback when no embedded date exists."""
    return datetime.fromtimestamp(path.stat().st_mtime, tz=timezone.utc).date().isoformat()


# Pagination fallback titles from extract/pdf.py ("Section (pp 6–9)") carry no meaning for
# retrieval or navigation (2026-06-05 evaluation); the summary pass's semantic title replaces
# them. Real extractor headings are never overridden. Page ranges live in section_map.
_PSEUDO_TITLE = re.compile(r"^Section \(pp \d+[–-]\d+\)$")


def _needs_semantic_title(title: str | None) -> bool:
    return title is None or bool(_PSEUDO_TITLE.match(title))


def _vec_blob(vec: list[float]) -> bytes:
    """Pack a float vector as the little-endian float32 blob sqlite-vec stores."""
    return struct.pack(f"{len(vec)}f", *vec)


def _copy_to_raw(path: Path, content_hash: str) -> str:
    """Copy the source into the flat raw store under a hash-named file; return that name."""
    raw_store = load().paths.raw_store
    raw_store.mkdir(parents=True, exist_ok=True)
    name = f"{content_hash}{path.suffix.lower()}"
    dest = raw_store / name
    if not dest.exists():
        shutil.copy2(path, dest)
    return name


def _copy_figures_to_raw(content_hash: str, figures: list["_PreparedFigure"]) -> None:
    """Write figure PNGs into the flat raw store as '{content_hash}_fig{N}.png', in place.

    Called AFTER prepare succeeds and BEFORE _write opens its transaction (the bytes come
    FROM extraction, so the source-file ordering of _copy_to_raw is impossible here).
    Deterministic names keyed on the source hash make the write idempotent: re-ingesting
    identical content rewrites the same files. Orphans from a *changed* source (new hash)
    are removed by the caller using the replaced doc's stored figures.raw_path rows.
    """
    raw_store = load().paths.raw_store
    raw_store.mkdir(parents=True, exist_ok=True)
    for fig in figures:
        name = f"{content_hash}_fig{fig.position}.png"
        dest = raw_store / name
        if not dest.exists():
            dest.write_bytes(fig.image_bytes)
        fig.raw_name = name


def _figure_raw_paths(conn, doc_id: int) -> list[str]:
    """The stored raw-store PNG names of a document's figures (for orphan cleanup)."""
    return [
        r["raw_path"] for r in conn.execute("SELECT raw_path FROM figures WHERE doc_id=?", (doc_id,))
    ]


def _unlink_figure_files(names: list[str], keep: set[str]) -> None:
    """Best-effort removal of replaced figure PNGs from the raw store, after commit."""
    raw_store = load().paths.raw_store
    for name in names:
        if name and name not in keep:
            try:
                (raw_store / name).unlink(missing_ok=True)
            except OSError as exc:  # cleanup must never fail an ingest
                log.warning("could not remove replaced figure file %s: %s", name, exc)


# --- per-source-type pass profile ----------------------------------------------------------
# Which LLM passes run for a source type (§2.6 keeps RETRIEVAL unified; ingest passes may
# vary, like video's transcript pre-pass). Code skips propositions (claims-from-raw-code is
# the weakest 8B task, §11.B — raw chunks + file summaries carry retrieval, §15.0) and the
# LLM entity pass (the extractor supplies exact AST entities for free).


@dataclass(frozen=True)
class _PassProfile:
    propositions: bool = True
    llm_entities: bool = True
    code_prompts: bool = False  # source-file summary + repository synthesis prompt variants
    # Figure capture + VLM description (step 11): only formats with a visual layer the text
    # extractor cannot represent. False elsewhere so audit knows 0 figures is BY DESIGN.
    figures: bool = False


_DEFAULT_PROFILE = _PassProfile()
_PASS_PROFILE: dict[str, _PassProfile] = {
    "code": _PassProfile(propositions=False, llm_entities=False, code_prompts=True),
    "pdf": _PassProfile(figures=True),
    "slides": _PassProfile(figures=True),
}


def pass_profile(source_type: str) -> _PassProfile:
    """The ingest pass profile for a source type. Public so eval/audit can mirror reality:
    a zero-prop code section is BY DESIGN, not a quality flag."""
    return _PASS_PROFILE.get(source_type, _DEFAULT_PROFILE)


# --- the heavy, no-DB-write work: extract -> LLM passes -> embeddings ----------------------


@dataclass
class _PreparedSection:
    position: int
    title: str | None
    page_start: int
    page_end: int
    summary: str
    summary_vec: list[float]
    chunks: list[str]
    chunk_vecs: list[list[float]]
    propositions: list[str]
    proposition_vecs: list[list[float]]
    entities: list  # list[entities.Entity]
    # code provenance (None for prose formats)
    file_path: str | None = None
    call_graph: dict | None = None
    chunk_provenance: list[PreChunk] | None = None  # parallel to `chunks` when present


@dataclass
class _PreparedFigure:
    position: int  # order within the document
    section_position: int | None  # ExtractedSection.position of the enclosing section
    page: int  # 1-based page / slide number
    kind: str  # 'raster' | 'vector' | 'slide'
    caption: str | None
    description: str | None  # VLM output; None when the pass failed QC (caption-only figure)
    index_vec: list[float] | None  # embedding of caption+description; None when both absent
    image_bytes: bytes
    raw_name: str | None = None  # set by _copy_figures_to_raw before _write


@dataclass
class _Prepared:
    title: str | None
    source_date: str | None  # ISO 'YYYY-MM-DD' from the source's metadata; None if absent
    synthesis: synthesis.DocSynthesis
    gaps: list[str]
    sections: list[_PreparedSection]
    embed_model: str
    figures: list[_PreparedFigure]


def _extract(path: Path, source_type: str) -> pdf_extract.ExtractedDoc:
    """Dispatch to the per-format extractor. Everything downstream is format-agnostic (§2.6)."""
    if source_type == "pdf":
        # mathocr=True / figures=True: the pipeline (unlike ad-hoc extraction) runs the
        # math-OCR pass on pages the damage/math detector flags (config [mathocr];
        # engine='off' disables) and detects+renders figure regions (config [figures]).
        return pdf_extract.extract_pdf(path, mathocr=True, figures=True)
    if source_type == "docx":
        return docx_extract.extract_docx(path)
    if source_type == "markdown":
        return textdoc.extract_markdown(path)
    if source_type == "text":
        return textdoc.extract_text(path)
    if source_type == "notebook":
        return textdoc.extract_notebook(path)
    if source_type == "slides":
        return pptx_extract.extract_pptx(path, figures=True)
    raise ValueError(f"no extractor for source_type {source_type!r}")


class _SummaryCache:
    """Content-keyed cache over the summary pass (pass_cache table, migration 0005).

    Keyed by sha256(file_blob_sha : summary : ingest_model : PROMPT_VERSION) — so a repo
    re-ingest only re-runs summaries for files whose bytes changed, and a model or prompt
    change invalidates naturally. Reads happen during prepare (read-only, WAL-safe);
    staged writes are flushed AFTER the document transaction succeeds (the cache is an
    optimization, not corpus data — it must never bloat or roll back the doc write).
    """

    def __init__(self, conn, blob_shas: dict[str, str]):
        self._conn = conn
        self._blob_shas = blob_shas
        self._puts: list[tuple[str, str]] = []

    def _key(self, sec: ExtractedSection) -> str | None:
        blob = self._blob_shas.get(sec.file_path or "")
        if not blob:
            return None
        raw = f"{blob}:summary:{llm.ingest_model()}:{summarize.PROMPT_VERSION}"
        return hashlib.sha256(raw.encode()).hexdigest()

    def get(self, sec: ExtractedSection) -> summarize.SectionSummary | None:
        key = self._key(sec)
        if key is None:
            return None
        row = self._conn.execute("SELECT payload FROM pass_cache WHERE key=?", (key,)).fetchone()
        return summarize.SectionSummary.model_validate_json(row["payload"]) if row else None

    def stage(self, sec: ExtractedSection, summary: summarize.SectionSummary) -> None:
        key = self._key(sec)
        if key is not None:
            self._puts.append((key, summary.model_dump_json()))

    def flush(self) -> None:
        """Persist staged entries (own transaction; call after the doc write commits)."""
        if self._puts:
            with self._conn:
                self._conn.executemany(
                    "INSERT OR REPLACE INTO pass_cache (key, payload) VALUES (?,?)", self._puts
                )
            self._puts = []


class _FigureCache:
    """Content-keyed cache over the figure-description pass (pass_cache table, 0005).

    Keyed by sha256(image_bytes) : figure : model : PROMPT_VERSION — image-content keyed,
    so re-ingesting an unchanged document re-runs ZERO VLM calls, and a model or prompt
    change invalidates naturally. A QC-failed description is cached as "" (tried, failed
    deterministically at temperature 0 + one retry; a prompt bump is the retry lever).
    Same read/stage/flush contract as _SummaryCache.

    `model` is the EFFECTIVE model string of the engine that actually produced (or will
    produce) the description — `figures.model` for the Ollama engine (existing keys stay
    valid), `figures.llamacpp_model` for the llama.cpp engine — so engines never reuse
    each other's outputs and a mid-batch fallback caches truthfully (step 11.6).
    """

    def __init__(self, conn):
        self._conn = conn
        self._puts: list[tuple[str, str]] = []

    @staticmethod
    def _key(image_bytes: bytes, model: str | None = None) -> str:
        raw = (
            f"{hashlib.sha256(image_bytes).hexdigest()}:figure:"
            f"{model or load().figures.model}:{figures_pass.PROMPT_VERSION}"
        )
        return hashlib.sha256(raw.encode()).hexdigest()

    def get(self, image_bytes: bytes, model: str | None = None) -> str | None:
        """Cached description ("" = cached failure), or None when never attempted."""
        row = self._conn.execute(
            "SELECT payload FROM pass_cache WHERE key=?", (self._key(image_bytes, model),)
        ).fetchone()
        return row["payload"] if row else None

    def stage(self, image_bytes: bytes, description: str, model: str | None = None) -> None:
        self._puts.append((self._key(image_bytes, model), description))

    def flush(self) -> None:
        """Persist staged entries (own transaction; call after the doc write commits)."""
        if self._puts:
            with self._conn:
                self._conn.executemany(
                    "INSERT OR REPLACE INTO pass_cache (key, payload) VALUES (?,?)", self._puts
                )
            self._puts = []


def _describe_figures(
    doc_figures: list, figure_cache: _FigureCache | None, gap_list: list[str]
) -> list[str | None]:
    """Engine-dispatched figure-description batch with fail-closed fallback (step 11.6).

    `engine="llamacpp"` spawns one llama-server for the doc's batch (GPU vision encode;
    the server's __enter__ owns the VRAM handoff via unload_all). ANY lifecycle failure —
    binary absent, startup timeout, server death mid-batch — degrades the remaining
    figures to the Ollama engine with a doc gap line; a dead server must never quarantine
    a document. Cache entries are keyed by the engine that actually produced them.
    """
    cfg = load().figures
    descriptions: list[str | None] = [None] * len(doc_figures)
    todo = list(range(len(doc_figures)))

    def run_engine(indices: list[int], client, model_key: str) -> list[int]:
        """Describe figures at `indices`; returns the indices left undone (server died)."""
        for n, i in enumerate(indices):
            fig = doc_figures[i]
            cached = figure_cache.get(fig.image_bytes, model_key) if figure_cache else None
            if cached is not None:
                descriptions[i] = cached or None  # "" = cached QC failure
                continue
            try:
                desc = figures_pass.describe_figure(fig.image_bytes, fig.caption, client=client)
            except Exception as exc:
                # Lifecycle failure (llamacpp.LlamaServerError) — hand the rest back for
                # fallback. Per-call errors never reach here (describe_figure degrades
                # them to None itself).
                log.warning("figure VLM engine failed mid-batch: %s", exc)
                return indices[n:]
            if figure_cache is not None:
                figure_cache.stage(fig.image_bytes, desc or "", model_key)
            descriptions[i] = desc
        return []

    if cfg.engine == "llamacpp" and todo:
        from locus.ingest import llamacpp  # lazy, mirrors the mathocr import style

        try:
            with llamacpp.LlamaServer(cfg) as server:
                todo = run_engine(todo, server, cfg.llamacpp_model)
        except llamacpp.LlamaServerError as exc:
            log.warning("llamacpp figure engine unavailable: %s", exc)
        if todo:
            gap_list.append(
                f"figure VLM engine 'llamacpp' unavailable for {len(todo)} figure(s); "
                "fell back to ollama"
            )
    if todo:
        # Ollama path (default engine, or llamacpp fallback). Explicit handoff: evict the
        # text model BEFORE the VLM loads — loading the ~6 GB VLM while Ollama is still
        # evicting the ~6.4 GB text model gets planned with a KEPT CPU/GPU split
        # (observed live 2026-06-06: 74/26 for a whole figure batch).
        llm.unload()
        run_engine(todo, None, cfg.model)
    return descriptions


def _prepare(path: Path, source_type: str, figure_cache: _FigureCache | None = None) -> _Prepared:
    doc = _extract(path, source_type)
    # VRAM choreography before the text passes (the mirror of the text->VLM handoff in
    # _prepare_doc): evict the PREVIOUS document's figures VLM if it is still resident —
    # otherwise the ~6.4 GB text model loads while Ollama evicts the ~6 GB VLM under
    # pressure and the planner allocates a kept CPU/GPU split. Then clear any model that
    # is already split (e.g. residue of the OCR engine's VRAM window). Both are safe
    # no-ops when nothing is resident.
    llm.unload(load().figures.model)
    llm.unload_if_split()
    return _prepare_doc(doc, source_type, figure_cache=figure_cache)


def _run_optional_pass(fn, sec, what: str, empty, pass_gaps: list[str]):
    """Run a non-critical pass; on persistent LLM failure degrade to empty + a gap flag.

    Propositions/entities are enhancements on top of the raw chunks + summary (§15.0:
    retrieval assembles raw text regardless) — one stubborn section must not quarantine a
    whole document. The summary pass stays fatal: it is the L2 retrieval unit itself.
    """
    try:
        return fn(sec.title, sec.text)
    except llm.IngestExtractionError as exc:
        log.warning("%s pass failed for section %r: %s", what, sec.title, exc)
        pass_gaps.append(f"{what} pass failed for section {sec.position} ({sec.title})")
        return empty


def _prepare_section(
    sec, profile: "_PassProfile", summary_cache: "_SummaryCache | None", pass_gaps: list[str]
) -> _PreparedSection:
    """Prepare one section: summary (cached) + chunks + propositions/entities + embeddings.

    Pure per-section work — no DB writes, no synthesis. Shared by full-document ingest
    (_prepare_doc) and incremental repo re-ingest (repo_sync), so a changed file is prepared
    identically whether the whole doc or one file is being (re)ingested.
    """
    sec_summary = summary_cache.get(sec) if summary_cache else None
    if sec_summary is None:
        sec_summary = summarize.summarize_section(sec.title, sec.text, code=profile.code_prompts)
        if summary_cache:
            summary_cache.stage(sec, sec_summary)
    if not sec_summary.grounded:
        # The model twice produced a summary about something other than this section
        # (the round-3 hallucination finding); a deterministic fallback replaced it.
        # Surface that as a doc gap so audit/inspect show the degradation.
        pass_gaps.append(
            f"summary failed grounding for section {sec.position} ({sec.title}); "
            "deterministic fallback used"
        )
    summary = sec_summary.summary
    # Pagination pseudo-titles get the summary pass's semantic title instead.
    title = sec.title
    if _needs_semantic_title(title) and sec_summary.title and sec_summary.title.strip():
        title = sec_summary.title.strip()
    if profile.propositions:
        props = _run_optional_pass(
            # has_math routes the math-dense prompt variant (claims-in-words for formulas).
            lambda t, x, _m=sec.has_math: propositions.extract_propositions(t, x, math_heavy=_m),
            sec, "propositions", [], pass_gaps,
        )
    else:
        props = []
    if sec.entities is not None:
        # Extractor-supplied (deterministic, e.g. code AST) — validated into the closed
        # EntityType vocabulary; never sent through the LLM pass.
        ents = [entities.Entity(name=e.name, type=e.type) for e in sec.entities]
    elif profile.llm_entities:
        ents = _run_optional_pass(entities.extract_entities, sec, "entities", [], pass_gaps)
    else:
        ents = []
    if sec.chunks is not None:
        chunks = [pc.text for pc in sec.chunks]
        chunk_provenance = sec.chunks
    else:
        chunks = chunk.chunk_text(sec.text)
        chunk_provenance = None
    return _PreparedSection(
        position=sec.position,
        title=title,
        page_start=sec.page_start,
        page_end=sec.page_end,
        summary=summary,
        summary_vec=embed.embed_text(summary),
        chunks=chunks,
        chunk_vecs=embed.embed_texts(chunks),
        propositions=props,
        proposition_vecs=embed.embed_texts(props),
        entities=ents,
        file_path=sec.file_path,
        call_graph=sec.call_graph,
        chunk_provenance=chunk_provenance,
    )


def _prepare_doc(
    doc: ExtractedDoc,
    source_type: str,
    summary_cache: _SummaryCache | None = None,
    figure_cache: _FigureCache | None = None,
) -> _Prepared:
    profile = _PASS_PROFILE.get(source_type, _DEFAULT_PROFILE)
    prepared: list[_PreparedSection] = []
    summaries: list[str] = []

    pass_gaps: list[str] = []

    for sec in doc.sections:
        ps = _prepare_section(sec, profile, summary_cache, pass_gaps)
        prepared.append(ps)
        summaries.append(ps.summary)
        log.info("section %s/%s done (%s)", sec.position + 1, len(doc.sections), sec.title)

    # Doc-level entity hygiene: collapse plural/singular surface variants (evidence-based).
    # Only for LLM-extracted entities — AST names are exact, and two functions legitimately
    # differing by a trailing 's' (chunk/chunks) must never merge.
    if profile.llm_entities:
        entities.merge_plural_variants([p.entities for p in prepared])

    syn = synthesis.synthesize_document(
        doc.title, summaries, code=profile.code_prompts,
        source_name=Path(doc.source_path).name if doc.source_path else None,
    )
    context = (
        f"Thesis: {syn.thesis}\nMethod: {syn.method}\n"
        f"Result: {syn.result}\nLimitations: {syn.limitations}"
    )
    # Evidence-grounded gap pass: section summaries show what IS covered; raw text is scanned
    # for explicit deferral phrases (confirmed gaps). See gaps.flag_gaps.
    gap_list = gaps.flag_gaps(
        doc.title, context,
        sections=[(p.title, p.summary, s.text) for p, s in zip(prepared, doc.sections)],
    )
    # Persist the math-OCR audit trail (extract/mathocr.py): pages where QC kept the original
    # text are extraction gaps in the §15.0 sense — content quality was knowingly degraded
    # there, and that must be queryable, not just logged.
    gap_list += [f"math-OCR kept original text on {f}" for f in doc.ocr_fallbacks]
    gap_list += pass_gaps  # sections whose optional passes degraded to empty

    # Figure-description pass, batched AFTER all text passes: the VLM takes the card
    # once per document instead of thrashing per section (the 8 GB VRAM choreography).
    # Descriptions come from the content-keyed cache when the image bytes were seen
    # before; embedding runs as one batch afterwards.
    prepared_figs: list[_PreparedFigure] = []
    if doc.figures and profile.figures and load().figures.enabled:
        descriptions = _describe_figures(doc.figures, figure_cache, gap_list)
        index_texts = [
            figures_pass.index_text(f.caption, d) for f, d in zip(doc.figures, descriptions)
        ]
        vecs = embed.embed_texts([t for t in index_texts if t])
        vec_iter = iter(vecs)
        for i, (fig, desc, text) in enumerate(zip(doc.figures, descriptions, index_texts)):
            prepared_figs.append(
                _PreparedFigure(
                    position=i,
                    section_position=fig.section_position,
                    page=fig.page,
                    kind=fig.kind,
                    caption=fig.caption,
                    description=desc,
                    index_vec=next(vec_iter) if text else None,
                    image_bytes=fig.image_bytes,
                )
            )
        failed = sum(1 for d in descriptions if d is None)
        if failed:
            gap_list.append(f"figure description failed QC for {failed}/{len(doc.figures)} figures")
        log.info("figures: %d described, %d caption-only", len(doc.figures) - failed, failed)
    # Degraded figure capture (e.g. slides without LibreOffice) is an extraction gap (§15.0).
    gap_list += [f"figure capture degraded: {f}" for f in doc.figure_fallbacks]
    if doc.ocr_replaced:
        log.info("math-OCR replaced %d page(s); %d fallback(s)",
                 len(doc.ocr_replaced), len(doc.ocr_fallbacks))
    # Title arbitration (see DocSynthesis.title + pdf.title_is_suspect): the synthesis pass's
    # title is used ONLY when the extractor's candidate looks like a heuristic accident —
    # a trusted title (publisher metadata, real heading) is never rewritten by the model.
    title = doc.title
    if pdf_extract.title_is_suspect(title) and (syn.title or "").strip():
        title = syn.title.strip()
        log.info("title corrected: %r -> %r", doc.title, title)
    return _Prepared(
        title, doc.source_date, syn, gap_list, prepared, embed.embedding_model(), prepared_figs
    )


# --- transactional write -----------------------------------------------------------------


def _write(
    conn, path: Path, content_hash: str, source_type: str, raw_name: str, prep: _Prepared,
    *, category: str | None = None, source_uri: str | None = None,
    replace_doc_id: int | None = None,
) -> IngestResult:
    """Write the prepared document in ONE transaction.

    `replace_doc_id` deletes the prior document inside the same transaction — so a
    re-ingest is atomic: the old doc survives unless the new one commits. (All expensive
    LLM work happened in prepare, before this opens.)
    """
    section_map = [
        {"position": s.position, "title": s.title, "page_start": s.page_start, "page_end": s.page_end}
        for s in prep.sections
    ]
    # Embedded source date if the extractor found one, else the file's mtime (always non-null).
    source_date = prep.source_date or _mtime_date(path)
    category = category or _category(path)
    n_chunks = n_props = n_ents = 0
    pos_to_section_id: dict[int, int] = {}  # ExtractedSection.position -> sections.id (figures)

    with conn:  # one transaction: commit on success, rollback on any error
        if replace_doc_id is not None:
            _delete_document_rows(conn, replace_doc_id)
        cur = conn.execute(
            "INSERT INTO documents (content_hash, source_type, source_uri, raw_path, title, "
            "ingest_model, source_date, category, thesis, method, result, limitations, "
            "section_map, gap_flags) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                content_hash, source_type, source_uri or str(path), raw_name, prep.title,
                llm.ingest_model(), source_date, category,
                prep.synthesis.thesis, prep.synthesis.method, prep.synthesis.result,
                prep.synthesis.limitations, json.dumps(section_map), json.dumps(prep.gaps),
            ),
        )
        doc_id = cur.lastrowid

        for s in prep.sections:
            section_id, sc, sp, se = _insert_section(conn, doc_id, s, prep.embed_model)
            pos_to_section_id[s.position] = section_id
            n_chunks += sc
            n_props += sp
            n_ents += se

        for f in prep.figures:
            if f.raw_name is None:  # programming error: _copy_figures_to_raw must precede
                raise RuntimeError(f"figure {f.position} has no raw_name; copy step skipped")
            cur = conn.execute(
                "INSERT INTO figures (section_id, doc_id, position, page, kind, caption, "
                "description, raw_path, embed_model) VALUES (?,?,?,?,?,?,?,?,?)",
                (
                    pos_to_section_id.get(f.section_position), doc_id, f.position, f.page,
                    f.kind, f.caption, f.description, f.raw_name, prep.embed_model,
                ),
            )
            # A caption-less, description-failed figure has nothing to search on: stored
            # for tier-1 preservation (and the audit), but given no vector.
            if f.index_vec is not None:
                conn.execute(
                    "INSERT INTO figure_vectors (figure_id, embedding) VALUES (?,?)",
                    (cur.lastrowid, _vec_blob(f.index_vec)),
                )

    return IngestResult(
        str(path), "ingested", doc_id=doc_id, sections=len(prep.sections),
        chunks=n_chunks, propositions=n_props, entities=n_ents, figures=len(prep.figures),
    )


def _insert_section(conn, doc_id: int, s: _PreparedSection, embed_model: str) -> tuple[int, int, int, int]:
    """Insert one prepared section + its vectors/chunks/propositions/entities. No transaction
    (caller owns it). Returns (section_id, n_chunks, n_props, n_entities). Shared by the
    whole-doc write (_write) and the surgical incremental update (repo_sync)."""
    cur = conn.execute(
        "INSERT INTO sections (doc_id, position, title, summary, cross_section_deps, "
        "file_path, call_graph) VALUES (?,?,?,?,?,?,?)",
        (doc_id, s.position, s.title, s.summary, None, s.file_path,
         json.dumps(s.call_graph) if s.call_graph else None),
    )
    section_id = cur.lastrowid
    conn.execute(
        "INSERT INTO section_vectors (section_id, embedding) VALUES (?,?)",
        (section_id, _vec_blob(s.summary_vec)),
    )
    n_chunks = n_props = n_ents = 0
    for i, (text, vec) in enumerate(zip(s.chunks, s.chunk_vecs)):
        prov = s.chunk_provenance[i] if s.chunk_provenance else None
        cur = conn.execute(
            "INSERT INTO chunks (section_id, doc_id, position, raw_text, embed_model, "
            "file_path, line_start, line_end, video_timestamp) VALUES (?,?,?,?,?,?,?,?,?)",
            (section_id, doc_id, i, text, embed_model,
             prov.file_path if prov else None,
             prov.line_start if prov else None,
             prov.line_end if prov else None,
             prov.video_timestamp if prov else None),
        )
        conn.execute(
            "INSERT INTO chunk_vectors (chunk_id, embedding) VALUES (?,?)",
            (cur.lastrowid, _vec_blob(vec)),
        )
        n_chunks += 1
    for i, (text, vec) in enumerate(zip(s.propositions, s.proposition_vecs)):
        cur = conn.execute(
            "INSERT INTO propositions (section_id, doc_id, position, text, embed_model) "
            "VALUES (?,?,?,?,?)",
            (section_id, doc_id, i, text, embed_model),
        )
        conn.execute(
            "INSERT INTO proposition_vectors (proposition_id, embedding) VALUES (?,?)",
            (cur.lastrowid, _vec_blob(vec)),
        )
        n_props += 1
    for e in s.entities:
        cur = conn.execute(
            "INSERT OR IGNORE INTO entities (doc_id, section_id, name, type) VALUES (?,?,?,?)",
            (doc_id, section_id, e.name, str(e.type)),
        )
        n_ents += max(0, cur.rowcount)
    return section_id, n_chunks, n_props, n_ents


# --- public entry points -----------------------------------------------------------------


def _delete_section_rows(conn, section_id: int) -> None:
    """Delete ONE section and all its children (caller owns the transaction).

    Mirrors _delete_document_rows' discipline at section granularity for the incremental
    repo update. Two reasons everything is deleted EXPLICITLY rather than via FK cascade:
    sqlite-vec `vec0` tables carry no foreign key (orphan vectors collide on a reused row
    id), and `recursive_triggers` is off — so the chunks_fts sync trigger only fires on a
    DIRECT `DELETE FROM chunks`, not on a cascade from the parent section.
    """
    cids = [r["id"] for r in conn.execute("SELECT id FROM chunks WHERE section_id=?", (section_id,))]
    pids = [r["id"] for r in conn.execute("SELECT id FROM propositions WHERE section_id=?", (section_id,))]
    fids = [r["id"] for r in conn.execute("SELECT id FROM figures WHERE section_id=?", (section_id,))]
    conn.executemany("DELETE FROM chunk_vectors WHERE chunk_id=?", [(i,) for i in cids])
    conn.executemany("DELETE FROM proposition_vectors WHERE proposition_id=?", [(i,) for i in pids])
    conn.executemany("DELETE FROM figure_vectors WHERE figure_id=?", [(i,) for i in fids])
    conn.execute("DELETE FROM section_vectors WHERE section_id=?", (section_id,))
    conn.execute("DELETE FROM chunks WHERE section_id=?", (section_id,))  # explicit: fires FTS trigger
    conn.execute("DELETE FROM propositions WHERE section_id=?", (section_id,))
    conn.execute("DELETE FROM figures WHERE section_id=?", (section_id,))
    conn.execute("DELETE FROM entities WHERE section_id=?", (section_id,))
    conn.execute("DELETE FROM sections WHERE id=?", (section_id,))


def _delete_document_rows(conn, doc_id: int) -> None:
    """Delete a document's rows WITHOUT a transaction wrapper (caller owns the transaction).

    The L1/L2/L3 + entity tables cascade via ON DELETE CASCADE, but the sqlite-vec `vec0`
    vector tables are virtual and carry NO foreign key, so their rows must be removed
    explicitly — otherwise vectors orphan and a reused row id later collides on insert.
    """
    sids = [r["id"] for r in conn.execute("SELECT id FROM sections WHERE doc_id=?", (doc_id,))]
    cids = [r["id"] for r in conn.execute("SELECT id FROM chunks WHERE doc_id=?", (doc_id,))]
    pids = [r["id"] for r in conn.execute("SELECT id FROM propositions WHERE doc_id=?", (doc_id,))]
    fids = [r["id"] for r in conn.execute("SELECT id FROM figures WHERE doc_id=?", (doc_id,))]
    conn.executemany("DELETE FROM section_vectors WHERE section_id=?", [(i,) for i in sids])
    conn.executemany("DELETE FROM chunk_vectors WHERE chunk_id=?", [(i,) for i in cids])
    conn.executemany("DELETE FROM proposition_vectors WHERE proposition_id=?", [(i,) for i in pids])
    conn.executemany("DELETE FROM figure_vectors WHERE figure_id=?", [(i,) for i in fids])
    conn.execute("DELETE FROM documents WHERE id=?", (doc_id,))


def delete_document(conn, doc_id: int) -> None:
    """Delete a document and all of its rows (own transaction), plus its figure PNGs."""
    figure_files = _figure_raw_paths(conn, doc_id)
    with conn:
        _delete_document_rows(conn, doc_id)
    _unlink_figure_files(figure_files, keep=set())  # after commit; best-effort


def ingest_file(path: str | Path, conn, *, reingest: bool = False) -> IngestResult:
    """Ingest one file into the given DB connection. Never raises: failures are quarantined.

    If `reingest` is True, an already-present document (same content hash) is replaced —
    the old document is deleted inside the SAME transaction that writes the new one, so a
    failed prepare leaves the prior document untouched (prepare-first ordering; previously
    the delete ran before prepare and a failure destroyed the old doc).
    """
    path = Path(path)
    if not path.exists():
        return IngestResult(str(path), "quarantined", error="file not found")
    source_type = _source_type(path)
    if source_type is None:
        return IngestResult(str(path), "unsupported", error=f"unsupported type: {path.suffix}")
    try:
        content_hash = _hash_file(path)
        existing = conn.execute(
            "SELECT id, source_uri, category FROM documents WHERE content_hash=?", (content_hash,)
        ).fetchone()
        if existing and not reingest:
            return IngestResult(str(path), "skipped", doc_id=existing["id"])

        # Re-ingest metadata continuity: a replacement's bytes usually come from the raw
        # store, whose path carries neither the drop-folder category nor the original
        # source location — deriving them from the new path silently wipes the facets
        # (observed live: a raw-store re-ingest turned a 'paper' into 'uncategorized'
        # with a raw-store source_uri). Inherit from the replaced doc unless the new path
        # supplies real values: a deliberate re-drop into a category folder still wins,
        # and a non-raw-store path is a genuine new source location.
        category = source_uri = None
        if existing:
            derived = _category(path)
            category = derived if derived != "uncategorized" else existing["category"]
            in_raw_store = path.resolve().parent == load().paths.raw_store
            source_uri = existing["source_uri"] if in_raw_store else None

        raw_name = _copy_to_raw(path, content_hash)
        cache = _FigureCache(conn)
        prep = _prepare(path, source_type, figure_cache=cache)  # expensive work BEFORE any delete
        _copy_figures_to_raw(content_hash, prep.figures)
        old_figure_files = _figure_raw_paths(conn, existing["id"]) if existing else []
        result = _write(
            conn, path, content_hash, source_type, raw_name, prep,
            category=category, source_uri=source_uri,
            replace_doc_id=existing["id"] if existing else None,
        )
        cache.flush()  # only after the document committed
        # Replaced doc's figure PNGs: remove any not re-written under the new hash. (Same
        # content hash => same names => nothing to remove; changed file => orphans go.)
        _unlink_figure_files(old_figure_files, keep={f.raw_name for f in prep.figures})
        return result
    except Exception as exc:  # quarantine this doc, keep the batch alive (§6)
        log.warning("Quarantined %s: %s", path, exc)
        return IngestResult(str(path), "quarantined", error=str(exc))


def _write_repo_manifest(snap: code_extract.RepoSnapshot, repo: Path) -> str:
    """Raw-store entry for a tracked repo: a manifest JSON, not a copy.

    Deliberate exception to §6's copy-the-raw-file rule: a git repo IS its own durable raw
    store (every commit recoverable by sha), so Locus stores a pointer — repo path, HEAD,
    and the eligible-file blob list (which doubles as the pass-cache key material).
    LocusDrop snapshot drops, whose source tree is deleted after ingest, get a real
    tarball instead (see watcher.py).
    """
    raw_store = load().paths.raw_store
    raw_store.mkdir(parents=True, exist_ok=True)
    name = f"{snap.content_hash}.manifest.json"
    payload = {
        "source_type": "code",
        "repo_path": str(repo),
        "head": snap.head,
        "ingested_at": datetime.now(timezone.utc).isoformat(),
        "files": [{"path": rel, "blob_sha": snap.blob_shas[rel]} for rel in snap.files],
    }
    (raw_store / name).write_text(json.dumps(payload, indent=1))
    return name


def ingest_repo(
    repo_path: str | Path, conn, *, force: bool = False,
    source_uri: str | None = None, raw_name: str | None = None,
) -> IngestResult:
    """Ingest a code repository as ONE document (sections = files). Never raises.

    Idempotent by content: `content_hash` is the git HEAD sha (or a manifest hash for
    non-git trees), and the prior document is found by `source_uri` (stable across
    commits) — same content and not `force` is a skip; otherwise the old document is
    replaced atomically (prepare-first, delete+write in one transaction).

    `source_uri` defaults to the resolved repo path; LocusDrop snapshot drops pass a
    stable `locusdrop:<name>` key (their incoming path is transient) plus a tarball
    `raw_name` (their source tree is deleted after ingest).
    """
    repo = Path(repo_path).resolve()
    uri = source_uri or str(repo)
    try:
        snap = code_extract.collect_repo(repo)
        existing = conn.execute(
            "SELECT id, content_hash FROM documents WHERE source_uri=? AND source_type='code'",
            (uri,),
        ).fetchone()
        if existing and existing["content_hash"] == snap.content_hash and not force:
            return IngestResult(str(repo), "skipped", doc_id=existing["id"])

        if raw_name is None:
            raw_name = _write_repo_manifest(snap, repo)
        cache = _SummaryCache(conn, snap.blob_shas)
        doc = code_extract.extract_repo(repo, snap)
        prep = _prepare_doc(doc, "code", summary_cache=cache)  # expensive work, no DB writes
        result = _write(
            conn, repo, snap.content_hash, "code", raw_name, prep,
            category="project", source_uri=uri,
            replace_doc_id=existing["id"] if existing else None,
        )
        cache.flush()  # only after the document committed
        return result
    except Exception as exc:  # quarantine this repo, keep the batch alive (§6)
        log.warning("Quarantined repo %s: %s", repo, exc)
        return IngestResult(str(repo), "quarantined", error=str(exc))


def ingest_paths(paths: list[str | Path], conn=None, *, reingest: bool = False) -> list[IngestResult]:
    """Ingest several files into the configured DB (or a provided connection)."""
    own = conn is None
    if own:
        conn = get_connection(load().paths.db)
    try:
        return [ingest_file(p, conn, reingest=reingest) for p in paths]
    finally:
        if own:
            conn.close()
