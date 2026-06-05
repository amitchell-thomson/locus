"""Extract structured content from a code repository (stdlib ast, Python-first).

Produces one `ExtractedDoc` per repo: sections = files (title and `file_path` = the
repo-relative path, text = full verbatim source, §15.0), with:

  - **PreChunks at def granularity** for `.py` files: every top-level function/class gets
    a chunk with real `line_start`/`line_end` from the AST (decorators included); module
    -level code between defs is grouped into contiguous-range chunks so no source line is
    unretrievable; oversized defs are line-split into sub-chunks with exact sub-spans.
  - **`call_graph`** per file: {qualified_def: [callee_names]} — name-level `ast.Call`
    collection (Name.id / dotted Attribute), no type inference. The LINK substrate (§1).
  - **Deterministic entities** (no LLM): top-level functions → type 'method', classes →
    'concept', class methods qualified as `Class.method` → 'method'. Pytest-style
    `test_*` defs are excluded (entity-table noise), and trivial `__init__.py` files
    (docstring/imports/`__all__` only) are skipped entirely.
  - `.md` files become plain-text sections (no ATX splitting — one section per file keeps
    `file_path` provenance clean; chunked by the generic token splitter downstream).
  - A file that fails `ast.parse` (SyntaxError) degrades to a plain-text section — logged,
    never fails the repo.

Two repo channels share this extractor (PLAN.md step 10): tracked server repos (git,
commit-sha content hash) and LocusDrop snapshot drops (possibly no .git — eligible files
fall back to a filesystem walk and the content hash to a manifest hash).
"""

from __future__ import annotations

import ast
import hashlib
import logging
import subprocess
from pathlib import Path
from pydantic import BaseModel

from locus.config import load
from locus.extract.base import ExtractedDoc, ExtractedSection, PreChunk, PreEntity, has_math
from locus.ingest.chunk import count_tokens

log = logging.getLogger(__name__)

# Eligible source suffixes. .py gets the AST treatment; .md rides along as plain text.
ALLOWED_SUFFIXES = {".py", ".md", ".markdown"}
# Vendored/generated dir names skipped in both the git and walk paths (belt-and-braces:
# git ls-files usually excludes these already, but committed vendored code exists).
EXCLUDED_DIR_PARTS = {
    "vendor", "node_modules", ".venv", "venv", "site-packages", "third_party",
    "dist", "build", "__pycache__",
}
MAX_FILE_BYTES = 1 << 20  # >1 MB "source" is a generated/data blob


class RepoSnapshot(BaseModel):
    """The content identity of a repo at extraction time.

    `content_hash` is the git HEAD sha when available (a true content hash of tree +
    history, and the free sync change-check), else a deterministic sha256 over the sorted
    (rel_path, blob_sha) manifest — so an unchanged re-drop of a non-git tree hash-skips.
    `blob_shas` double as the pass-cache keys (ingest_pipeline).
    """

    head: str | None  # git HEAD sha, None for non-git trees
    files: list[str]  # repo-relative eligible files, sorted
    blob_shas: dict[str, str]  # rel_path -> sha256 of file bytes
    content_hash: str
    source_date: str | None  # last-commit date (git) or None (mtime fallback downstream)


def repo_head(repo: Path) -> str | None:
    """The repo's HEAD commit sha, or None if `repo` is not itself a git work-tree root.

    `git rev-parse` walks UP the directory tree, so a plain folder nested inside some
    other repo (e.g. a LocusDrop tree under vault/incoming/, which lives inside the locus
    repo) would silently resolve to the ENCLOSING repo's HEAD — and `git ls-files` would
    then list nothing for the gitignored drop. Requiring toplevel == repo rejects that.
    """
    repo = Path(repo).resolve()
    try:
        out = subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "--show-toplevel", "HEAD"],
            capture_output=True, text=True, check=True, timeout=30,
        )
        toplevel, _, head = out.stdout.strip().partition("\n")
        if Path(toplevel).resolve() != repo:
            return None  # inside someone else's work tree, not a repo itself
        return head.strip() or None
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, FileNotFoundError):
        return None


def _git_files(repo: Path) -> list[str] | None:
    """Tracked files via `git ls-files` (respects .gitignore), or None if not a git repo."""
    try:
        out = subprocess.run(
            ["git", "-C", str(repo), "ls-files", "-z"],
            capture_output=True, text=True, check=True, timeout=30,
        )
        return [f for f in out.stdout.split("\0") if f]
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, FileNotFoundError):
        return None


def _walk_files(repo: Path) -> list[str]:
    """Filesystem fallback for non-git trees (LocusDrop snapshot drops)."""
    out: list[str] = []
    for p in sorted(repo.rglob("*")):
        if not p.is_file():
            continue
        rel = p.relative_to(repo)
        if any(part.startswith(".") for part in rel.parts):
            continue  # dotted dirs/files (.git, .DS_Store, ...)
        out.append(str(rel))
    return out


def _eligible(repo: Path, rel: str) -> bool:
    p = Path(rel)
    if p.suffix.lower() not in ALLOWED_SUFFIXES:
        return False
    if p.name.endswith(".lock"):
        return False
    if any(part in EXCLUDED_DIR_PARTS for part in p.parts):
        return False
    full = repo / p
    try:
        size = full.stat().st_size
    except OSError:
        return False
    return 0 < size <= MAX_FILE_BYTES


def _git_last_commit_date(repo: Path) -> str | None:
    """Date of the last commit (ISO YYYY-MM-DD) — 'when was this last worked on'."""
    try:
        out = subprocess.run(
            ["git", "-C", str(repo), "log", "-1", "--format=%cs"],
            capture_output=True, text=True, check=True, timeout=30,
        )
        return out.stdout.strip() or None
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, FileNotFoundError):
        return None


def collect_repo(repo: Path) -> RepoSnapshot:
    """Snapshot the repo's eligible files + content identity (no LLM, no AST yet)."""
    repo = Path(repo).resolve()
    head = repo_head(repo)
    listed = _git_files(repo) if head else None
    files = sorted(rel for rel in (listed if listed is not None else _walk_files(repo)) if _eligible(repo, rel))
    if not files:
        raise ValueError(f"no eligible source files in {repo}")
    blob_shas = {
        rel: hashlib.sha256((repo / rel).read_bytes()).hexdigest() for rel in files
    }
    if head:
        content_hash = head
    else:
        manifest = "\n".join(f"{rel}:{blob_shas[rel]}" for rel in files)
        content_hash = hashlib.sha256(manifest.encode()).hexdigest()
    return RepoSnapshot(
        head=head, files=files, blob_shas=blob_shas, content_hash=content_hash,
        source_date=_git_last_commit_date(repo) if head else None,
    )


def extract_repo(repo: str | Path, snapshot: RepoSnapshot | None = None) -> ExtractedDoc:
    """Extract a structured ExtractedDoc from the repository at `repo`."""
    repo = Path(repo).resolve()
    snap = snapshot or collect_repo(repo)
    sections: list[ExtractedSection] = []
    for rel in snap.files:
        source = (repo / rel).read_text(encoding="utf-8", errors="replace")
        if _is_trivial_init(rel, source):
            # Boilerplate `__init__.py` (docstring/imports/__all__ only) would become a
            # full section with an LLM summary saying nothing — entity-table and section
            # noise, never a useful retrieval target (2026-06-05 round-3 evaluation).
            continue
        position = len(sections)
        if Path(rel).suffix.lower() == ".py":
            sections.append(_py_section(position, rel, source))
        else:
            sections.append(
                ExtractedSection(
                    position=position, title=rel, text=source, page_start=1, page_end=1,
                    has_math=has_math(source), file_path=rel,
                )
            )
    if not sections:
        raise ValueError(f"no non-trivial source files in {repo}")
    return ExtractedDoc(
        # Repo dir name: spaceless => title_is_suspect => the repo synthesis pass's
        # arbitrated title replaces it downstream with a semantic one.
        title=repo.name,
        page_count=0,
        section_strategy="repo",
        sections=sections,
        source_path=str(repo),
        source_date=snap.source_date,
    )


# --- per-file AST extraction ---------------------------------------------------------------


def _is_trivial_init(rel: str, source: str) -> bool:
    """True for an `__init__.py` carrying no content of its own.

    Trivial = only a docstring, imports/re-exports, and dunder assignments (`__all__`,
    `__version__`). An `__init__.py` that defines real functions/classes or module logic
    is NOT trivial and ingests like any other module.
    """
    if Path(rel).name != "__init__.py":
        return False
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return False  # let _py_section handle (and log) the broken file
    for node in tree.body:
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            continue
        if (
            isinstance(node, ast.Expr)
            and isinstance(node.value, ast.Constant)
            and isinstance(node.value.value, str)
        ):
            continue  # docstring
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            if all(
                isinstance(t, ast.Name) and t.id.startswith("__") and t.id.endswith("__")
                for t in targets
            ):
                continue  # __all__ / __version__ / similar dunder metadata
        return False
    return True


def _py_section(position: int, rel: str, source: str) -> ExtractedSection:
    """One ExtractedSection per .py file: def-granular PreChunks + call_graph + entities."""
    try:
        tree = ast.parse(source)
    except SyntaxError as exc:
        log.warning("syntax error in %s (%s): ingesting as plain text", rel, exc)
        return ExtractedSection(
            position=position, title=rel, text=source, page_start=1, page_end=1,
            has_math=False, file_path=rel,
        )
    lines = source.splitlines()
    budget = load().embed.chunk_tokens
    chunks = _chunk_module(tree, lines, rel, budget)
    return ExtractedSection(
        position=position, title=rel, text=source, page_start=1, page_end=1,
        has_math=False, file_path=rel,
        call_graph=_call_graph(tree),
        chunks=chunks,
        entities=_ast_entities(tree),
    )


def _node_start(node: ast.AST) -> int:
    """First source line of a def/class, including its decorators."""
    decorators = getattr(node, "decorator_list", [])
    return min([node.lineno] + [d.lineno for d in decorators])


_DEF_NODES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)


def _chunk_module(tree: ast.Module, lines: list[str], rel: str, budget: int) -> list[PreChunk]:
    """Def-granular chunks covering every line of the module.

    Top-level defs/classes become chunks (a class that fits the budget is one chunk; an
    oversized one recurses into its methods). Module-level statements between defs are
    grouped into contiguous-range chunks. Oversized leaves are line-split with exact
    sub-spans. Coverage invariant: chunk spans tile [1, len(lines)] with no gaps.
    """
    boundaries: list[tuple[int, int, ast.AST | None]] = []  # (start, end, def_node|None)
    cursor = 1
    for node in tree.body:
        if isinstance(node, _DEF_NODES):
            start = _node_start(node)
            if start > cursor:
                boundaries.append((cursor, start - 1, None))  # interstitial module code
            boundaries.append((start, node.end_lineno, node))
            cursor = node.end_lineno + 1
    if cursor <= len(lines):
        boundaries.append((cursor, len(lines), None))

    chunks: list[PreChunk] = []
    for start, end, node in boundaries:
        if isinstance(node, ast.ClassDef) and _span_tokens(lines, start, end) > budget:
            chunks.extend(_chunk_class(node, lines, rel, budget))
        else:
            chunks.extend(_emit_span(lines, start, end, rel, budget))
    return chunks


def _chunk_class(node: ast.ClassDef, lines: list[str], rel: str, budget: int) -> list[PreChunk]:
    """An oversized class: chunk per method, interstitial class-level lines grouped."""
    chunks: list[PreChunk] = []
    cursor = _node_start(node)
    for item in node.body:
        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
            start = _node_start(item)
            if start > cursor:
                chunks.extend(_emit_span(lines, cursor, start - 1, rel, budget))
            chunks.extend(_emit_span(lines, start, item.end_lineno, rel, budget))
            cursor = item.end_lineno + 1
    if cursor <= node.end_lineno:
        chunks.extend(_emit_span(lines, cursor, node.end_lineno, rel, budget))
    return chunks


def _span_tokens(lines: list[str], start: int, end: int) -> int:
    return count_tokens("\n".join(lines[start - 1 : end]))


def _emit_span(lines: list[str], start: int, end: int, rel: str, budget: int) -> list[PreChunk]:
    """Emit [start, end] (1-based, inclusive) as one PreChunk, line-splitting if oversized."""
    text = "\n".join(lines[start - 1 : end])
    if not text.strip():
        return []
    if count_tokens(text) <= budget:
        return [PreChunk(text=text, file_path=rel, line_start=start, line_end=end)]
    # Oversized: greedy line packing with exact sub-spans.
    chunks: list[PreChunk] = []
    seg_start = start
    seg: list[str] = []
    seg_tokens = 0
    for n in range(start, end + 1):
        line = lines[n - 1]
        ltokens = count_tokens(line) + 1
        if seg and seg_tokens + ltokens > budget:
            chunks.append(
                PreChunk(text="\n".join(seg), file_path=rel, line_start=seg_start, line_end=n - 1)
            )
            seg, seg_tokens, seg_start = [], 0, n
        seg.append(line)
        seg_tokens += ltokens
    if seg and "\n".join(seg).strip():
        chunks.append(
            PreChunk(text="\n".join(seg), file_path=rel, line_start=seg_start, line_end=end)
        )
    return chunks


def _call_graph(tree: ast.Module) -> dict[str, list[str]] | None:
    """{qualified_def: [callee_names]} — name-level only, best effort."""
    graph: dict[str, list[str]] = {}
    for qualname, node in _iter_defs(tree):
        if isinstance(node, ast.ClassDef):
            continue  # only callables; a class entry would double-count its methods' calls
        callees: list[str] = []
        seen: set[str] = set()
        for sub in ast.walk(node):
            if isinstance(sub, ast.Call):
                name = _callee_name(sub.func)
                if name and name not in seen:
                    seen.add(name)
                    callees.append(name)
        if callees:
            graph[qualname] = callees
    return graph or None


def _callee_name(func: ast.AST) -> str | None:
    if isinstance(func, ast.Name):
        return func.id
    if isinstance(func, ast.Attribute):
        base = _callee_name(func.value)
        return f"{base}.{func.attr}" if base else func.attr
    return None


def _iter_defs(tree: ast.Module):
    """(qualified_name, node) for top-level defs/classes and one level of class methods."""
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            yield node.name, node
        elif isinstance(node, ast.ClassDef):
            yield node.name, node
            for item in node.body:
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    yield f"{node.name}.{item.name}", item


def _is_test_def(name: str) -> bool:
    """Pytest-convention test methods: never useful entity targets, and a test file emits
    dozens of them (36 in one file, 2026-06-05 round-3 evaluation), drowning the entity
    table. They stay retrievable through their chunks (lexical + dense); they just don't
    get entity rows."""
    return name.split(".")[-1].startswith("test_")


def _ast_entities(tree: ast.Module) -> list[PreEntity] | None:
    """Deterministic entities: functions -> 'method', classes -> 'concept'.

    Test methods (`test_*`) are excluded — see _is_test_def."""
    out: list[PreEntity] = []
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if not _is_test_def(node.name):
                out.append(PreEntity(name=node.name, type="method"))
        elif isinstance(node, ast.ClassDef):
            out.append(PreEntity(name=node.name, type="concept"))
            for item in node.body:
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    if not _is_test_def(item.name):
                        out.append(PreEntity(name=f"{node.name}.{item.name}", type="method"))
    return out or None
