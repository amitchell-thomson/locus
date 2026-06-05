"""Step 10: repo extraction — eligible files, AST chunks/spans, call graph, entities.

Synthetic repos built in tmp_path; git-dependent tests are skipped when git is missing.
"""

import shutil
import subprocess
from pathlib import Path

import pytest

from locus.extract.code import RepoSnapshot, collect_repo, extract_repo, repo_head

HAS_GIT = shutil.which("git") is not None
needs_git = pytest.mark.skipif(not HAS_GIT, reason="git not installed")

PY_MAIN = '''\
"""Module docstring."""

import math

CONSTANT = 17.3


def alpha(x):
    """Doubles x."""
    return beta(x) * 2


def beta(x):
    return math.sqrt(x)


class Engine:
    """A class with methods."""

    def run(self):
        return alpha(1)

    def stop(self):
        return self.run()
'''

PY_BROKEN = "def broken(:\n    pass\n"


def _make_tree(root: Path) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    (root / "main.py").write_text(PY_MAIN)
    (root / "broken.py").write_text(PY_BROKEN)
    (root / "README.md").write_text("# My Project\n\nIt computes things end to end.\n")
    (root / "uv.lock").write_text("locked")
    (root / "data.csv").write_text("a,b\n1,2\n")
    sub = root / "__pycache__"
    sub.mkdir()
    (sub / "main.cpython-311.pyc").write_text("x")
    return root


def _git(repo: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.email=t@t", "-c", "user.name=t", *args],
        check=True, capture_output=True,
    )


def _make_git_repo(root: Path) -> Path:
    _make_tree(root)
    _git(root, "init", "-q")
    _git(root, "add", "-A")
    _git(root, "commit", "-qm", "initial")
    return root


def test_walk_path_eligibility_without_git(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    snap = collect_repo(repo)
    assert snap.head is None
    assert snap.files == ["README.md", "broken.py", "main.py"]  # lock/csv/pycache excluded
    assert len(snap.content_hash) == 64  # manifest hash, not a commit sha
    # Deterministic: same tree -> same hash.
    assert collect_repo(repo).content_hash == snap.content_hash


@needs_git
def test_git_path_uses_head_and_tracked_files(tmp_path: Path):
    repo = _make_git_repo(tmp_path / "gitrepo")
    snap = collect_repo(repo)
    assert snap.head == repo_head(repo)
    assert snap.content_hash == snap.head
    assert "main.py" in snap.files and "uv.lock" not in snap.files
    assert snap.source_date is not None  # last-commit date


@needs_git
def test_non_git_tree_nested_inside_a_git_repo_uses_walk(tmp_path: Path):
    """Regression (live 2026-06-05): `git rev-parse` walks UP, so a plain drop folder
    nested inside another repo resolved to the ENCLOSING repo's HEAD and ls-files (in the
    gitignored drop) returned nothing -> 'no eligible source files'. repo_head must require
    the directory to be its own work-tree root."""
    outer = _make_git_repo(tmp_path / "outer")
    drop = _make_tree(outer / "vault" / "incoming" / "projects" / "drop")
    assert repo_head(drop) is None  # NOT the outer repo's HEAD
    snap = collect_repo(drop)
    assert snap.head is None
    assert "main.py" in snap.files  # walk path found the files


def test_sections_and_def_granular_chunks(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    assert doc.page_count == 0 and doc.section_strategy == "repo"
    assert [s.title for s in doc.sections] == ["README.md", "broken.py", "main.py"]

    main = next(s for s in doc.sections if s.title == "main.py")
    assert main.file_path == "main.py"
    assert main.chunks is not None
    # Defs map to chunks with real line spans (alpha starts at its def line).
    src_lines = PY_MAIN.splitlines()
    alpha_def_line = src_lines.index("def alpha(x):") + 1
    alpha_chunk = next(c for c in main.chunks if c.text.startswith("def alpha"))
    assert alpha_chunk.line_start == alpha_def_line
    assert alpha_chunk.line_end > alpha_def_line
    assert alpha_chunk.file_path == "main.py"
    # Coverage: every line of the file falls inside some chunk span.
    covered = set()
    for c in main.chunks:
        covered.update(range(c.line_start, c.line_end + 1))
    nonblank = {i + 1 for i, line in enumerate(src_lines) if line.strip()}
    assert nonblank <= covered


def test_call_graph_and_entities(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    main = next(s for s in doc.sections if s.title == "main.py")
    assert "beta" in main.call_graph["alpha"]
    assert "alpha" in main.call_graph["Engine.run"]
    assert "self.run" in main.call_graph["Engine.stop"]
    assert "Engine" not in main.call_graph  # classes are not callables in the graph
    ents = {(e.name, e.type) for e in main.entities}
    assert ("alpha", "method") in ents
    assert ("Engine", "concept") in ents
    assert ("Engine.run", "method") in ents


def test_syntax_error_degrades_to_plain_text(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    broken = next(s for s in doc.sections if s.title == "broken.py")
    assert broken.chunks is None  # falls back to generic chunk_text downstream
    assert broken.call_graph is None
    assert "def broken" in broken.text


def test_readme_is_plain_text_section(tmp_path: Path):
    repo = _make_tree(tmp_path / "plain")
    doc = extract_repo(repo)
    readme = next(s for s in doc.sections if s.title == "README.md")
    assert readme.chunks is None and readme.call_graph is None
    assert "computes things" in readme.text


def test_oversized_def_line_splits_with_exact_subspans(tmp_path: Path):
    body = "\n".join(f"    x{i} = compute_value({i}) + compute_value({i + 1})" for i in range(400))
    (tmp_path / "big").mkdir()
    (tmp_path / "big" / "big.py").write_text(f"def huge():\n{body}\n    return x0\n")
    doc = extract_repo(tmp_path / "big")
    sec = doc.sections[0]
    assert len(sec.chunks) > 1  # split
    spans = [(c.line_start, c.line_end) for c in sec.chunks]
    # Sub-spans are contiguous and ordered.
    for (s1, e1), (s2, _) in zip(spans, spans[1:]):
        assert s2 == e1 + 1
    assert spans[0][0] == 1


def test_empty_repo_raises(tmp_path: Path):
    empty = tmp_path / "empty"
    empty.mkdir()
    (empty / "data.csv").write_text("a,b\n")
    with pytest.raises(ValueError, match="no eligible source files"):
        collect_repo(empty)


def test_repo_title_is_dir_name(tmp_path: Path):
    repo = _make_tree(tmp_path / "myproject")
    doc = extract_repo(repo)
    assert doc.title == "myproject"  # suspect (no spaces) -> synthesis arbitrates downstream
