"""Corpus-level document retitling (give docs distinctive, content-representative titles).

The ingest title pass (ingest/synthesis.py) sees only the extractor's candidate + the
section summaries — never the filename (sequence), the folder path (module/subject), or
sibling documents (collisions). So multi-part series and generic metadata exports collapse
to one title: 8 ODE lectures all "P1 Ordinary Differential Equations 1", 10 decks all
"PowerPoint Presentation", 7 CVs all "Alec Mitchell-Thomson".

This pass runs AFTER ingest batches (like `locus link`): it has the global view collision-
breaking needs, and it rewrites only `documents.title` — no re-ingest, no touch to
sections/chunks/vectors/synthesis. The composed title is:

    [Module — ][Seq: ]Topic

  - Module: the deepest meaningful parent folder of source_uri ('Ordinary Differential
            Equations 1', 'Calculus 3'); omitted when the parent is just a category bucket.
  - Seq:    a sequence marker parsed deterministically from the filename ('Lecture 6').
  - Topic:  the distinctive content, distilled from thesis/method/result by the Claude API
            (judgement-quality, §11.B), cached in pass_cache by content hash. The model
            keeps an already-faithful specific title VERBATIM (papers, real titles survive).

A final deterministic collision-break guarantees uniqueness (suffix by date, then a folder
hint, then the doc id). Idempotent and re-runnable; a sidecar snapshot of prior titles is
written before any write so `locus retitle --rollback` restores them.
"""

from __future__ import annotations

import json
import logging
import re
import time
from dataclasses import dataclass, field
from pathlib import Path

from pydantic import BaseModel

from locus.config import load
from locus.eval.judge import _anthropic_client

log = logging.getLogger(__name__)

# Bump to invalidate cached topics in pass_cache (prompt/schema changes).
PROMPT_VERSION = 1

# Sidecar holding the pre-run titles, for --rollback. One file, overwritten each run.
BACKUP_NAME = "title_backup.json"

# Folder names that are structural buckets, never a document's "module". Lowercased match.
# Categories (singular+plural) + drop-zone scaffolding + year buckets are handled separately.
_BUCKET_FOLDERS = {
    "incoming", "vault", "raw", "notes", "note", "papers", "paper", "coursework",
    "career", "careers", "project", "projects", "career-documents", "course_files",
}
_YEAR_FOLDER = re.compile(r"^(oxford-)?year[\s_-]*\d+$", re.IGNORECASE)

# Filename sequence markers -> normalised label. Order matters: first match wins.
_SEQ_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"lecture[\s_-]*0*(\d+)", re.IGNORECASE), "Lecture {}"),
    (re.compile(r"\bweek[\s_-]*0*(\d+)", re.IGNORECASE), "Week {}"),
    (re.compile(r"\bpart[\s_-]*0*(\d+)", re.IGNORECASE), "Part {}"),
    (re.compile(r"\bsheet[\s_-]*0*(\d+)", re.IGNORECASE), "Sheet {}"),
    (re.compile(r"\bsession[\s_-]*0*(\d+)", re.IGNORECASE), "Session {}"),
    # Bare 'L<n>' only inside a token like 'P1-Calc3-L1' (hyphen/underscore bounded), so it
    # never fires on an ordinary word. Checked last — the explicit words above win.
    (re.compile(r"[-_]L0*(\d+)\b", re.IGNORECASE), "Lecture {}"),
]
# 'Examples'/'Worked' companion sheets — a sub-marker appended to the sequence in parens.
_EXAMPLES = re.compile(r"\b(examples?|worked|solutions?)\b", re.IGNORECASE)


@dataclass
class RetitleReport:
    total: int = 0
    changed: int = 0
    kept: int = 0  # title already good — left verbatim
    collisions_broken: int = 0
    api_calls: int = 0
    cache_hits: int = 0
    failed: int = 0  # docs whose topic distill errored — stored title kept
    proposals: list[tuple[int, str, str]] = field(default_factory=list)  # (id, old, new)

    def summary(self) -> str:
        return (
            f"retitle: {self.total} docs | changed {self.changed} | kept {self.kept} | "
            f"collisions broken {self.collisions_broken} | failed {self.failed} | "
            f"llm: {self.api_calls} calls, {self.cache_hits} cache hits"
        )


class TopicVerdict(BaseModel):
    """The distilled topic for one document.

    keep_existing=True means the stored title is already a faithful, specific, non-generic
    title (a real paper/document title) and must be used verbatim with no module/sequence
    decoration. topic is a short content phrase otherwise.
    """

    keep_existing: bool
    topic: str


_SYSTEM = (
    "You title documents in a personal knowledge base so each is findable and distinct. "
    "You are given one document's stored synthesis (thesis/method/result), its source "
    "filename, and its folder. Decide:\n"
    "- If the EXISTING title is already a faithful, specific, non-generic title of THIS "
    "document (e.g. a real paper or report title), set keep_existing=true and copy it as "
    "topic verbatim.\n"
    "- Otherwise set keep_existing=false and give a SHORT topic phrase (at most 8 words, no "
    "trailing punctuation) naming the document's distinctive content — what makes it "
    "different from a sibling on the same course. Do NOT include the module name, a lecture "
    "number, the word 'lecture', or the person's name — those are added separately. Generic "
    "metadata ('PowerPoint Presentation', 'Untitled', a course-module banner repeated across "
    "every lecture, or a bare person name) is NOT a faithful title — distil a real topic from "
    "the synthesis instead.\n"
    "Be faithful to the synthesis; never invent content not implied by it. Use the title tool."
)


def _build_user(*, existing: str, filename: str, folder: str, thesis: str, method: str, result: str) -> str:
    return (
        f"Existing title: {existing!r}\n"
        f"Filename: {filename}\n"
        f"Folder: {folder or '(none)'}\n\n"
        f"Thesis: {thesis}\nMethod: {method}\nResult: {result}"
    )


def distill_topic(
    *, existing: str, filename: str, folder: str, thesis: str, method: str, result: str,
    client=None, model: str | None = None, max_tokens: int = 256,
) -> TopicVerdict:
    """Distil a document's topic with Claude (forced tool-use). Raises if no result returns."""
    client = client or _anthropic_client()
    model = model or load().generation.model
    tool = {
        "name": "title",
        "description": "Record the distilled topic for this document.",
        "input_schema": TopicVerdict.model_json_schema(),
    }
    resp = client.messages.create(
        model=model,
        max_tokens=max_tokens,
        system=_SYSTEM,
        tools=[tool],
        tool_choice={"type": "tool", "name": "title"},
        messages=[{"role": "user", "content": _build_user(
            existing=existing, filename=filename, folder=folder,
            thesis=thesis, method=method, result=result,
        )}],
    )
    for block in resp.content:
        if getattr(block, "type", None) == "tool_use":
            return TopicVerdict.model_validate(_unwrap(block.input))
    raise RuntimeError("Retitle topic distiller returned no tool_use block")


def _unwrap(raw):
    """Recover the occasional double-encoded tool input.

    ~1 in 200 responses nests the whole verdict as a JSON STRING inside the `topic`
    field and omits `keep_existing` ({'topic': '{"keep_existing":...,"topic":"..."}'}),
    which would fail validation and (pre-fix) abort the run. If the top-level dict is
    missing keep_existing but its topic parses as a verdict, use the inner object.
    """
    if isinstance(raw, dict) and "keep_existing" not in raw and isinstance(raw.get("topic"), str):
        try:
            inner = json.loads(raw["topic"])
        except (json.JSONDecodeError, TypeError):
            return raw
        if isinstance(inner, dict) and "keep_existing" in inner:
            return inner
    return raw


# --- deterministic signal extraction ------------------------------------------------------


def parse_sequence(filename: str) -> str | None:
    """A normalised sequence label from a filename ('Lecture 6'), or None.

    Appends an '(Examples)' sub-marker when the filename also reads as a companion sheet,
    so 'P1-Calc3-L1-Examples' -> 'Lecture 1 (Examples)' sits distinct from 'P1-Calc3-L1'.
    """
    stem = Path(filename).stem
    for pattern, label in _SEQ_PATTERNS:
        m = pattern.search(stem)
        if m:
            seq = label.format(int(m.group(1)))
            if _EXAMPLES.search(stem):
                seq += " (Examples)"
            return seq
    return None


def module_context(source_uri: str) -> str | None:
    """The deepest meaningful folder of a document WITHIN the drop tree, or None.

    Anchored at the 'incoming' drop root so it never climbs into filesystem scaffolding
    above it (a paper in papers/ must not get '/home/alec/.../locus' as its module). Walks up
    the in-drop folders skipping structural buckets (category folders, 'year2'-style buckets)
    and returns the first real one — the course module ('Ordinary Differential Equations 1').
    A file sitting directly in a category bucket, or any path with no 'incoming' anchor (e.g.
    a tracked-repo source path), has no module.
    """
    parts = Path(source_uri.rstrip("/")).parts[:-1]  # drop the filename
    lowered = [p.lower() for p in parts]
    if "incoming" not in lowered:
        return None
    in_drop = parts[len(lowered) - 1 - lowered[::-1].index("incoming") + 1:]  # folders after incoming
    for part in reversed(in_drop):
        if part.lower() in _BUCKET_FOLDERS or _YEAR_FOLDER.match(part) or part in ("/", ""):
            continue
        return part.strip()
    return None


def _clean_repo_name(source_uri: str) -> str:
    """A display name for a code repo from its path/uri, used as the title's leading slot:
    'optiver-trading-academy' -> 'Optiver Trading Academy', 'locusdrop:alpha-fund' ->
    'Alpha Fund'. Already-uppercase tokens (acronyms) are preserved; the rest are
    capitalised. Repos otherwise lose their identity to a name-less description."""
    base = source_uri.rstrip("/").split("/")[-1].split(":", 1)[-1]  # drop a 'locusdrop:' prefix
    words = [w for w in re.split(r"[-_\s]+", base) if w]
    return " ".join(w if w.isupper() else w.capitalize() for w in words)


def compose(module: str | None, seq: str | None, topic: str) -> str:
    """Assemble [Module — ][Seq: ]Topic from the available slots."""
    topic = topic.strip()
    if module and seq:
        return f"{module} — {seq}: {topic}"
    if module:
        # A topic that already leads with the module name (a repo whose description names
        # itself) must not be doubled — 'Locus — Locus self-hosted...' -> 'Locus self-hosted...'.
        if topic.lower().startswith(module.lower()):
            return topic
        return f"{module} — {topic}"
    if seq:
        return f"{seq}: {topic}"
    return topic


def _break_collisions(items: list[tuple[int, str, str, str | None]], report: RetitleReport) -> dict[int, str]:
    """Guarantee unique titles. items: (doc_id, composed_title, source_date, source_uri).

    Identical composed titles get a deterministic suffix — source_date, then a folder hint,
    then the doc id as a last resort — so the corpus never holds two same-titled docs.
    """
    by_title: dict[str, list[tuple[int, str, str | None]]] = {}
    for doc_id, title, date, uri in items:
        by_title.setdefault(title, []).append((doc_id, date, uri))
    out: dict[int, str] = {}
    for title, group in by_title.items():
        if len(group) == 1:
            out[group[0][0]] = title
            continue
        report.collisions_broken += len(group)
        used: set[str] = set()
        for doc_id, date, uri in group:
            for suffix in (
                f" ({date})" if date else "",
                f" [{Path(uri).stem}]" if uri else "",
                f" (#{doc_id})",
            ):
                cand = f"{title}{suffix}" if suffix else title
                if cand and cand not in used:
                    break
            used.add(cand)
            out[doc_id] = cand
    return out


# --- orchestration -------------------------------------------------------------------------


def _topic_cache_key(content_hash: str, model: str) -> str:
    return f"{content_hash}:retitle:{model}:{PROMPT_VERSION}"


def _backup_path() -> Path:
    return load().paths.db.parent / BACKUP_NAME


def rollback(conn, *, log=log.info) -> int:
    """Restore titles from the most recent pre-run sidecar snapshot. Returns rows restored."""
    path = _backup_path()
    if not path.exists():
        log(f"retitle: no backup at {path}")
        return 0
    saved = json.loads(path.read_text())
    n = 0
    for doc_id, title in saved.items():
        conn.execute("UPDATE documents SET title=? WHERE id=?", (title, int(doc_id)))
        n += 1
    conn.commit()
    log(f"retitle: restored {n} titles from {path}")
    return n


def build_titles(
    conn,
    *,
    use_llm: bool | None = None,
    use_cache: bool = True,
    dry_run: bool = False,
    client=None,
    log=log.info,
) -> RetitleReport:
    """Recompute distinctive titles for every document and (unless dry_run) write them.

    use_llm=None -> config [retitle].use_llm. With use_llm False the topic falls back to the
    thesis's leading clause (deterministic, free). Topics are cached in pass_cache keyed on
    content_hash + model + PROMPT_VERSION, so re-runs after new ingests only call the API for
    docs not seen before; use_cache=False re-distils every doc.
    """
    cfg = load().retitle
    if use_llm is None:
        use_llm = cfg.use_llm
    model = load().generation.model
    if use_llm and client is None:
        client = _anthropic_client()

    rows = conn.execute(
        "SELECT id, title, source_uri, source_date, content_hash, source_type, "
        "COALESCE(thesis,'') thesis, COALESCE(method,'') method, COALESCE(result,'') result "
        "FROM documents ORDER BY id"
    ).fetchall()
    report = RetitleReport(total=len(rows))

    last_api = 0.0
    composed: list[tuple[int, str, str, str | None]] = []  # (id, title, date, uri)

    for r in rows:
        if r["source_type"] == "code":
            # A repo has no incoming-anchored module, and a name-less description loses the
            # project's identity ('self-hosted knowledge base' for what is Locus). Use the
            # repo name as the module slot so the title leads with it ('Locus — ...'); the
            # LLM topic stays the description. seq is meaningless for a repo.
            module, seq = _clean_repo_name(r["source_uri"]), None
        else:
            module = module_context(r["source_uri"])
            seq = parse_sequence(Path(r["source_uri"].rstrip("/")).name)

        if not use_llm:
            # The deterministic path has no way to tell a good title from a banner, so it
            # only rewrites docs with a real structural signal (a module folder or a filename
            # sequence) — the coursework series this mode exists for. Bucket-level docs
            # (papers, CVs) keep their stored title rather than get clobbered by a thesis
            # fragment; the LLM path is what reworks those.
            if module is None and seq is None:
                title = r["title"]
            else:
                title = compose(module, seq, _fallback_topic(r["thesis"], r["title"]))
        else:
            try:
                verdict, last_api = _topic_for(
                    r, model, use_cache, client, report, conn, dry_run=dry_run,
                    throttle=cfg.api_call_interval, last_api=last_api,
                )
                title = verdict.topic.strip() if verdict.keep_existing else compose(module, seq, verdict.topic)
            except Exception as exc:
                # One unparseable/failed response must not abort 300+ docs (and waste the
                # API spend) — keep this doc's stored title and carry on. The doc-level
                # collision-break below still guarantees uniqueness.
                report.failed += 1
                log(f"retitle: topic distill failed for doc {r['id']} ({r['title']!r}): {exc}; keeping title")
                title = r["title"]
        composed.append((r["id"], title, r["source_date"], r["source_uri"]))

    final = _break_collisions(composed, report)

    for r in rows:
        new = final[r["id"]]
        if new != r["title"]:
            report.changed += 1
            report.proposals.append((r["id"], r["title"], new))
        else:
            report.kept += 1

    if not dry_run:
        snapshot = {str(r["id"]): r["title"] for r in rows}
        _backup_path().write_text(json.dumps(snapshot, indent=0))
        conn.executemany(
            "UPDATE documents SET title=? WHERE id=?",
            [(final[r["id"]], r["id"]) for r in rows if final[r["id"]] != r["title"]],
        )
        conn.commit()
    log(report.summary())
    return report


def _topic_for(r, model, use_cache, client, report, conn, *, dry_run, throttle, last_api):
    """Resolve one document's TopicVerdict via cache or a throttled API call.

    On a cache miss the freshly distilled verdict is persisted to pass_cache immediately, in
    its own commit (unless dry_run), so a crash never discards a billed topic. Returns
    (verdict, last_api) — last_api is the monotonic timestamp of the most recent real API
    call, advanced only on a cache miss so the caller can space subsequent calls.
    """
    key = _topic_cache_key(r["content_hash"], model)
    if use_cache:
        hit = conn.execute("SELECT payload FROM pass_cache WHERE key=?", (key,)).fetchone()
        if hit:
            report.cache_hits += 1
            return TopicVerdict.model_validate_json(hit["payload"]), last_api
    if throttle > 0 and last_api:
        wait = throttle - (time.monotonic() - last_api)
        if wait > 0:
            time.sleep(wait)
    verdict = distill_topic(
        existing=r["title"], filename=Path(r["source_uri"].rstrip("/")).name,
        folder=module_context(r["source_uri"]) or "", thesis=r["thesis"],
        method=r["method"], result=r["result"], client=client, model=model,
    )
    report.api_calls += 1
    # Persist immediately (own commit) so a crash never discards a billed topic — a re-run
    # resumes from cache instead of re-paying. dry_run stays side-effect-free (no cache write).
    if not dry_run:
        with conn:
            conn.execute(
                "INSERT OR REPLACE INTO pass_cache (key, payload) VALUES (?,?)",
                (key, verdict.model_dump_json()),
            )
    return verdict, time.monotonic()


def _fallback_topic(thesis: str, existing: str) -> str:
    """Deterministic topic when use_llm is off: the thesis's leading clause, else the title.

    Strips a leading 'To <verb>' purpose preamble ('To explain frequency response ...' ->
    'frequency response ...') and clips to a short phrase. A crude stand-in for the LLM.
    """
    t = thesis.strip()
    t = re.sub(r"^to\s+\w+\s+", "", t, flags=re.IGNORECASE)  # drop 'To explain '
    t = re.split(r"[.;:]", t, maxsplit=1)[0].strip()
    words = t.split()
    if len(words) > 9:
        t = " ".join(words[:9])
    return (t[:1].upper() + t[1:]) if t else existing.strip()
