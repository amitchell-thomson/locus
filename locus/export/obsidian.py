"""Obsidian projection — a one-way, regenerable render of the SQLite corpus as a vault (§13).

This is a *render target*, never a data source: retrieval never reads it, and re-export is
`rm -rf` + recompute == identity. The design and its invariants live in
docs/obsidian-projection-plan.md; the load-bearing ones, restated as code constraints:

  1. One-way / regenerable — we only ever WRITE under `out_dir`; nothing is read back.
  2. Joins-only — every node and edge comes from a deterministic query (no LLM, no inference).
     Reuses `link/related.py` for the doc<->doc edges, exactly as `locus inspect` renders them.
  3. Canonical entities only — entity notes join through `entity_aliases`, never raw surfaces.
  4. Owns only its subtrees — the exporter writes/prunes ONLY `docs/` and `entities/` (and
     rewrites `_index.md`); it NEVER touches `.obsidian/` (the user's per-machine layout). The
     prune step therefore restricts its unlinks to those two subtrees by construction.

Transport to the Mac (where Obsidian's GUI runs) is an rsync pull that mirrors invariant #4 at
the transport layer — `rsync --delete --exclude '.obsidian/'`; see the plan's §10.

Rendering (`slug` / `doc_note_markdown` / `entity_note_markdown`) is pure string-in/string-out
and unit-tested with seeded rows; `export_vault` does query -> render -> write -> prune.
"""

from __future__ import annotations

import re
import sqlite3
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

from locus.link.related import aliases_built, related_documents, resolve_stop_doc_freq

# The exporter owns exactly these subtrees under out_dir. The prune step only ever unlinks
# *.md within them, so `.obsidian/`, `_index.md`, and anything else the user keeps in the
# vault root are structurally untouchable (invariant #4).
_OWNED_SUBDIRS = ("docs", "entities")

_SLUG_RE = re.compile(r"[^a-z0-9]+")


@dataclass(frozen=True)
class WikiLink:
    """An Obsidian `[[path|alias]]` link: stable slug target, human-readable label."""

    target: str  # vault-relative path, no extension (e.g. "docs/paper/foo-12")
    label: str

    def render(self) -> str:
        return f"[[{self.target}|{_inline(self.label)}]]"


@dataclass
class ExportReport:
    out_dir: Path
    doc_notes: int
    entity_notes: int
    related_edges: int
    pruned: int
    aliases_built: bool
    warnings: list[str] = field(default_factory=list)


# --------------------------------------------------------------------------- rendering (pure)


def slug(text: str, *, disambiguator: int | None = None) -> str:
    """Filesystem-safe slug from `text`; `disambiguator` (a stable id) suffixes on collision.

    Stable across re-exports because the disambiguator is an immutable id (doc id / cluster
    id), not a positional counter — so a note's filename (and thus Obsidian's graph identity)
    survives title changes and the arrival of new colliding docs.
    """
    base = _SLUG_RE.sub("-", (text or "").lower()).strip("-") or "untitled"
    return f"{base}-{disambiguator}" if disambiguator is not None else base


def _inline(text: str) -> str:
    """Make a label safe to sit inside `[[...|label]]` (no link-breaking chars, single line)."""
    return (
        (text or "")
        .replace("|", "/")
        .replace("]]", "]")
        .replace("\n", " ")
        .replace("\r", " ")
        .strip()
    )


def _yaml_scalar(value: object) -> str:
    """Always-quoted YAML scalar: synthesis fields carry colons/quotes/newlines that would
    otherwise break the frontmatter block. Single-line, double-quoted, backslash-escaped."""
    s = str(value).replace("\\", "\\\\").replace('"', '\\"')
    s = s.replace("\n", " ").replace("\r", " ").replace("\t", " ")
    return f'"{s}"'


def _frontmatter(pairs: list[tuple[str, object]]) -> str:
    lines = ["---"]
    lines += [f"{k}: {_yaml_scalar(v)}" for k, v in pairs if v is not None]
    lines.append("---")
    return "\n".join(lines)


def _get(row, key: str):
    """Read a key from a sqlite3.Row or a dict, returning None when the column is absent."""
    try:
        return row[key]
    except (KeyError, IndexError):
        return None


def doc_note_markdown(doc, sections, related: list[WikiLink], entities: list[WikiLink]) -> str:
    """Render one document note: synthesis frontmatter + section summaries + related/entity links.

    `doc` is a documents row (mapping); `sections` rows carry position/title/summary; `related`
    and `entities` are pre-resolved wikilinks (the orchestrator owns slug resolution).
    """
    title = _get(doc, "title") or "(untitled)"
    parts = [
        _frontmatter(
            [
                ("title", title),
                ("category", _get(doc, "category")),
                ("source_type", _get(doc, "source_type")),
                ("source_date", _get(doc, "source_date")),
                ("source_uri", _get(doc, "source_uri")),
                ("thesis", _get(doc, "thesis")),
                ("method", _get(doc, "method")),
                ("result", _get(doc, "result")),
                ("limitations", _get(doc, "limitations")),
            ]
        ),
        "",
        f"# {title}",
        "",
    ]
    for s in sections:
        parts.append(f"## {_get(s, 'position')}. {_get(s, 'title') or '(untitled section)'}")
        summary = (_get(s, "summary") or "").strip()
        parts.append(summary or "_(no summary)_")
        parts.append("")
    if related:
        parts.append("## Related documents")
        parts += [f"- {w.render()}" for w in related]
        parts.append("")
    if entities:
        parts.append("## Entities")
        parts += [f"- {w.render()}" for w in entities]
        parts.append("")
    return "\n".join(parts).rstrip() + "\n"


def entity_note_markdown(
    canonical_name: str,
    canonical_type: str,
    variants: list[str],
    mentioning_docs: list[WikiLink],
) -> str:
    """Render one canonical entity note: the variant surfaces it subsumes + doc backlinks.

    Joins through `entity_aliases` upstream — `canonical_name` is always a canonical surface,
    never a raw `entities.name` (invariant #3). Variant surfaces equal to the canonical are
    dropped (they add no information)."""
    parts = [
        _frontmatter(
            [
                ("canonical_name", canonical_name),
                ("canonical_type", canonical_type),
                ("kind", "entity"),
            ]
        ),
        "",
        f"# {canonical_name}",
        "",
        f"_Canonical {canonical_type} entity._",
        "",
    ]
    surfaces = [v for v in variants if v and v != canonical_name]
    if surfaces:
        parts.append("## Variant surfaces")
        parts += [f"- {s}" for s in surfaces]
        parts.append("")
    if mentioning_docs:
        parts.append("## Mentioned in")
        parts += [f"- {w.render()}" for w in mentioning_docs]
        parts.append("")
    return "\n".join(parts).rstrip() + "\n"


# ------------------------------------------------------------------------- orchestration (IO)


def _guard_out_dir(out_dir: Path, db_path: Path | None) -> None:
    """Refuse to operate on a root/empty out_dir, or one that contains the live DB.

    Belt-and-braces on top of the structural guarantee that prune only unlinks within
    `docs/`/`entities/`: the destination must be a real subtree, never `/` or a directory the
    DB lives under (invariant #4 / plan §5)."""
    resolved = out_dir.resolve()
    if not resolved.name or resolved.parent == resolved:
        raise ValueError(f"refusing to export to a filesystem root: {resolved}")
    if db_path is not None and resolved in Path(db_path).resolve().parents:
        raise ValueError(f"out_dir {resolved} is a parent of the DB — refusing (plan §5)")


def _category_dir(doc) -> str:
    return slug(_get(doc, "category") or "uncategorized")


def _assign_doc_paths(docs) -> dict[int, tuple[str, str]]:
    """doc id -> (vault-relative path without extension, category dir). Slugs collide only
    within a category dir; contested ones get the immutable doc id appended."""
    bases: dict[int, tuple[str, str]] = {}
    counts: Counter = Counter()
    for d in docs:
        cat = _category_dir(d)
        b = slug(_get(d, "title") or "untitled")
        bases[d["id"]] = (cat, b)
        counts[(cat, b)] += 1
    out: dict[int, tuple[str, str]] = {}
    for d in docs:
        cat, b = bases[d["id"]]
        s = slug(_get(d, "title") or "untitled", disambiguator=d["id"]) if counts[(cat, b)] > 1 else b
        out[d["id"]] = (f"docs/{cat}/{s}", cat)
    return out


def _assign_entity_paths(emitted_canon: dict) -> dict[tuple[str, str], str]:
    """(canonical_name, canonical_type) -> vault-relative path. Collisions within a type dir
    get the cluster id appended (stable per the alias substrate)."""
    bases: dict[tuple[str, str], tuple[str, str]] = {}
    counts: Counter = Counter()
    for (name, ctype) in emitted_canon:
        t, b = slug(ctype), slug(name)
        bases[(name, ctype)] = (t, b)
        counts[(t, b)] += 1
    out: dict[tuple[str, str], str] = {}
    for key, info in emitted_canon.items():
        t, b = bases[key]
        s = slug(key[0], disambiguator=info["cluster_id"]) if counts[(t, b)] > 1 else b
        out[key] = f"entities/{t}/{s}"
    return out


def _load_documents(conn: sqlite3.Connection, include_excluded: bool):
    """Exported document set, respecting `[retrieve].exclude_source_uris` unless overridden
    (the self-ingested locus repo stays out of the projection too, like in retrieval)."""
    excluded: list[str] = []
    if not include_excluded:
        from locus.config import load

        excluded = list(load().retrieve.exclude_source_uris)
    if excluded:
        ph = ",".join("?" * len(excluded))
        return conn.execute(
            f"SELECT * FROM documents WHERE source_uri NOT IN ({ph}) ORDER BY id", excluded
        ).fetchall()
    return conn.execute("SELECT * FROM documents ORDER BY id").fetchall()


def _load_canonicals(conn: sqlite3.Connection, doc_ids: list[int]) -> dict:
    """Map (canonical_name, canonical_type) -> {doc_ids, variants, cluster_id} over the
    EXPORTED docs only — so spans and backlinks never reference a doc note that doesn't exist."""
    if not doc_ids:
        return {}
    ph = ",".join("?" * len(doc_ids))
    rows = conn.execute(
        f"""
        SELECT a.canonical_name, a.canonical_type, a.cluster_id, e.doc_id, e.name AS variant
        FROM entities e
        JOIN entity_aliases a
          ON a.variant_name = e.name AND a.variant_type = e.type
        WHERE e.doc_id IN ({ph})
        """,
        doc_ids,
    ).fetchall()
    canon: dict = {}
    for r in rows:
        key = (r["canonical_name"], r["canonical_type"])
        info = canon.setdefault(key, {"doc_ids": set(), "variants": set(), "cluster_id": r["cluster_id"]})
        info["doc_ids"].add(r["doc_id"])
        info["variants"].add(r["variant"])
        info["cluster_id"] = min(info["cluster_id"], r["cluster_id"])
    return canon


def _write(out_dir: Path, relpath: str, content: str) -> None:
    path = out_dir / relpath
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def _prune(out_dir: Path, emitted: set[str]) -> int:
    """Remove stale notes: any *.md under the owned subtrees that this run did not emit (e.g. a
    deleted document's note). Scoped to `docs/`/`entities/`, so `.obsidian/` is never touched."""
    pruned = 0
    for sub in _OWNED_SUBDIRS:
        root = out_dir / sub
        if not root.exists():
            continue
        for p in sorted(root.rglob("*.md")):
            if p.relative_to(out_dir).as_posix() not in emitted:
                p.unlink()
                pruned += 1
    return pruned


def _index_markdown(docs, doc_paths, emitted_canon, entity_paths, related_map) -> str:
    """Generated TOC: counts, the biggest entity clusters, and orphan documents (no edges)."""
    linked: set[int] = set()
    edge_count = 0
    for did, rels in related_map.items():
        if rels:
            linked.add(did)
            linked.update(r.doc_id for r in rels)
            edge_count += len(rels)
    for info in emitted_canon.values():
        linked |= info["doc_ids"]
    orphans = [d for d in docs if d["id"] not in linked]

    lines = [
        "---",
        'title: "Locus corpus index"',
        "---",
        "",
        "# Locus corpus index",
        "",
        f"- **Documents:** {len(docs)}",
        f"- **Canonical entity notes:** {len(emitted_canon)}",
        f"- **Related-document edges:** {edge_count}",
        f"- **Orphans (no edges):** {len(orphans)}",
        "",
    ]
    if emitted_canon:
        lines.append("## Biggest entity clusters")
        ranked = sorted(emitted_canon.items(), key=lambda kv: (-len(kv[1]["doc_ids"]), kv[0]))
        for (name, ctype), info in ranked[:10]:
            lines.append(f"- {WikiLink(entity_paths[(name, ctype)], name).render()} — {len(info['doc_ids'])} docs")
        lines.append("")
    if orphans:
        lines.append("## Orphans")
        lines += [f"- {WikiLink(doc_paths[d['id']][0], _get(d, 'title') or '(untitled)').render()}" for d in orphans]
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def export_vault(
    conn: sqlite3.Connection,
    out_dir: Path,
    *,
    top_related: int = 5,
    include_excluded: bool = False,
    min_cluster_docs: int = 2,
    emit_entity_notes: bool = True,
    db_path: Path | None = None,
    log=lambda _m: None,
) -> ExportReport:
    """Render the corpus into `out_dir` as an Obsidian vault, then prune stale notes.

    Deterministic given the DB: stable slugs + sorted iteration => byte-identical re-exports
    and clean diffs. Joins-only; the doc<->doc edges reuse `related_documents` (so the graph's
    neighbours match `locus inspect`). Entity notes require the alias substrate (`locus link`);
    without it, the export degrades to a docs-only graph with a warning. Returns an
    ExportReport (counts + warnings)."""
    out_dir = Path(out_dir)
    _guard_out_dir(out_dir, db_path)
    warnings: list[str] = []

    docs = _load_documents(conn, include_excluded)
    docs_by_id = {d["id"]: d for d in docs}
    doc_ids = list(docs_by_id)
    id_set = set(doc_ids)
    if not docs:
        warnings.append("no documents to export")
    doc_paths = _assign_doc_paths(docs)

    built = aliases_built(conn)

    # Canonical entity nodes (Phase 2) — emitted only when they span >= min_cluster_docs of the
    # exported docs (a single-doc canonical draws no cross-doc edge; mirrors related.py).
    emitted_canon: dict = {}
    if emit_entity_notes:
        if built:
            canon = _load_canonicals(conn, doc_ids)
            emitted_canon = {k: v for k, v in canon.items() if len(v["doc_ids"]) >= min_cluster_docs}
        else:
            warnings.append("entity_aliases not built (run `locus link`) — emitting docs-only graph")
    entity_paths = _assign_entity_paths(emitted_canon)

    # doc -> entity mention links (to emitted canonicals only — no dangling links).
    doc_entity_links: dict[int, list[WikiLink]] = defaultdict(list)
    for (name, ctype), info in emitted_canon.items():
        for did in info["doc_ids"]:
            doc_entity_links[did].append(WikiLink(entity_paths[(name, ctype)], name))

    # doc <-> doc related edges — same IDF-weighted, stop-entity-guarded ranking as inspect.
    related_map: dict[int, list] = {}
    if built and top_related:
        stop = resolve_stop_doc_freq(conn)
        for d in docs:
            related_map[d["id"]] = [
                r for r in related_documents(conn, d["id"], top_n=top_related, stop_doc_freq=stop)
                if r.doc_id in id_set
            ]
    elif top_related and not built:
        warnings.append("entity_aliases not built — no related-document edges")

    log(f"  rendering {len(docs)} doc notes" + (f" + {len(emitted_canon)} entity notes" if emitted_canon else ""))

    emitted_paths: set[str] = set()
    related_edges = 0
    for d in docs:
        did = d["id"]
        relpath = doc_paths[did][0]
        sections = conn.execute(
            "SELECT position, title, summary FROM sections WHERE doc_id=? ORDER BY position", (did,)
        ).fetchall()
        related_links = [WikiLink(doc_paths[r.doc_id][0], r.title) for r in related_map.get(did, [])]
        related_edges += len(related_links)
        entity_links = sorted(doc_entity_links.get(did, []), key=lambda w: w.label.lower())
        _write(out_dir, relpath + ".md", doc_note_markdown(d, sections, related_links, entity_links))
        emitted_paths.add(relpath + ".md")

    for (name, ctype), info in sorted(emitted_canon.items()):
        relpath = entity_paths[(name, ctype)]
        mentions = sorted(
            (WikiLink(doc_paths[did][0], _get(docs_by_id[did], "title") or "(untitled)") for did in info["doc_ids"]),
            key=lambda w: w.label.lower(),
        )
        _write(out_dir, relpath + ".md", entity_note_markdown(name, ctype, sorted(info["variants"]), mentions))
        emitted_paths.add(relpath + ".md")

    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "_index.md").write_text(_index_markdown(docs, doc_paths, emitted_canon, entity_paths, related_map))

    pruned = _prune(out_dir, emitted_paths)
    if pruned:
        log(f"  pruned {pruned} stale note(s)")

    return ExportReport(
        out_dir=out_dir,
        doc_notes=len(docs),
        entity_notes=len(emitted_canon),
        related_edges=related_edges,
        pruned=pruned,
        aliases_built=built,
        warnings=warnings,
    )
