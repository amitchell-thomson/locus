"""Chats, repos and calendar become items; the runner never prunes after reading nothing (H1.3-H1.7)."""

from __future__ import annotations

import json


from locus.heuris import db as hdb
from locus.heuris import run as hrun
from locus.heuris.index import index
from locus.heuris.sources import calendar, chats, repos
from tests.test_heuris_index import _item, fake_embed


def _line(**kw):
    return json.dumps(kw)


def test_a_session_yields_your_prompts_and_claudes_prose_only(tmp_path):
    f = tmp_path / "sess.jsonl"
    cwd = "/Users/a/local-projects/quast"
    f.write_text("\n".join([
        _line(type="ai-title", aiTitle="barrier tuning"),
        _line(type="user", uuid="u1", timestamp="2026-11-01T10:00:00Z", cwd=cwd,
              message={"content": "why did the sweep fail?<system-reminder>noise</system-reminder>"}),
        _line(type="assistant", uuid="a1", timestamp="2026-11-01T10:00:05Z", cwd=cwd,
              message={"content": [{"type": "thinking", "thinking": "hmm"}, {"type": "tool_use", "name": "Bash"}]}),
        _line(type="user", uuid="r1", cwd=cwd, message={"content": [{"type": "tool_result", "content": "huge output"}]}),
        _line(type="assistant", uuid="a2", timestamp="2026-11-01T10:01:00Z", cwd=cwd,
              message={"content": [{"type": "text", "text": "The grid was too coarse."}]}),
        _line(type="assistant", uuid="a3", cwd=cwd, message={"content": [{"type": "text", "text": "Fixed it."}]}),
        _line(type="user", uuid="u2", isCompactSummary=True, message={"content": "This session is being continued"}),
        _line(type="user", uuid="u3", message={"content": "<command-name>/clear</command-name>"}),
    ]))
    its = chats.session_items(f, "laptop")
    assert [i.meta["role"] for i in its] == ["you", "claude"]
    assert its[0].text == "why did the sweep fail?" and its[0].thing == "quast"
    assert its[1].text == "The grid was too coarse.\n\nFixed it."
    assert its[1].ref == "chat:laptop/sess#a2" and its[0].meta["session_title"] == "barrier tuning"
    assert "huge output" not in " ".join(i.text for i in its)


def test_headless_tmp_sessions_are_never_read(tmp_path, monkeypatch):
    lap = tmp_path / "data" / "transcripts" / "laptop"
    (lap / "-proj").mkdir(parents=True)
    (lap / "-proj-tmp").mkdir()
    (lap / "-proj" / "a.jsonl").write_text("")
    (lap / "-proj-tmp" / "b.jsonl").write_text("")
    monkeypatch.setattr(chats, "roots", lambda d: [("laptop", lap)])
    conn = hdb.connect(tmp_path / "h.db")
    files = chats.changed_files(conn, tmp_path)
    assert [f.name for _, f, _, _ in files] == ["a.jsonl"]
    chats.mark_read(conn, files)
    assert chats.changed_files(conn, tmp_path) == []


def test_repo_docs_and_commits(tmp_path):
    (tmp_path / "quast.json").write_text(json.dumps({"id": "quast", "folder": "quast", "docs": [
        {"path": "README.md", "mtime": 1762000000, "text": "quast prices barriers"}], "commits": [
        {"sha": "abc", "at": "2026-11-08T10:00:00+00:00", "author": "Alec", "subject": "barrier sweep",
         "body": "", "files": ["sweep.py"], "repo": ""}]}))
    its = list(repos.items(tmp_path))
    assert {i.ref for i in its} == {"doc:quast/README.md", "commit:quast@abc"}
    assert all(i.thing == "quast" and i.area == "Projects" for i in its)


def test_family_events_get_the_family_area_and_tasks_their_thing():
    e = calendar.event_item({"uid": "U", "source": "family", "title": "Christmas", "recurring": False,
                             "span": {"type": "all_day", "start": "2026-12-25", "end": "2026-12-26"}})
    assert e.area == "Family" and e.kind == "event"
    t = calendar.task_item({"id": "t-1", "title": "plan", "status": "open", "locus": "quast",
                            "created_at": "2026-11-09T14:29:20.632843778Z"})
    assert t.thing == "quast" and t.area == "Projects"


def test_a_full_scan_that_reads_nothing_prunes_nothing(tmp_path, monkeypatch):
    conn = hdb.connect(tmp_path / "h.db")
    index(conn, "repos", [_item("doc:quast/README.md", kind="doc")], embed=fake_embed)
    monkeypatch.setattr(hrun, "inbox", lambda: tmp_path / "empty")
    o = hrun.run_source(conn, "repos", embed=fake_embed)
    assert o.error and "not pruning" in o.error
    assert conn.execute("SELECT COUNT(*) FROM items").fetchone()[0] == 1
    row = hrun.status(conn)
    assert next(r for r in row if r["source"] == "repos")["state"] == "error"


def test_one_source_failing_does_not_stop_the_others(tmp_path, monkeypatch):
    conn = hdb.connect(tmp_path / "h.db")
    monkeypatch.setattr(calendar, "items", lambda: (_ for _ in ()).throw(OSError("kairosd down")))
    (tmp_path / "in" / "repos").mkdir(parents=True)
    monkeypatch.setattr(hrun, "inbox", lambda: tmp_path / "in")
    test_repo_docs_and_commits(tmp_path / "in" / "repos")
    out = hrun.run(["calendar", "repos"], conn=conn, embed=fake_embed)
    assert "kairosd down" in out[0].error and out[1].error is None and out[1].stats.added == 2
