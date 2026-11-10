"""Slice 0.10: what changed is scrubbed into the staging tree, once; nothing unscrubbed is staged."""

import json

from locus import transcripts_push as tp


def test_changed_transcripts_are_scrubbed_once(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCUS_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.setattr(tp, "load_known", lambda: [])
    src = tmp_path / "projects" / "-Users-a-x"
    src.mkdir(parents=True)
    line = json.dumps({"type": "user", "message": {"content": "my password: hunter2hunter2"}})
    (src / "s1.jsonl").write_text(line + "\n")
    r = tp.push(tmp_path / "projects", send=False)
    assert r.scrubbed == 1 and r.redacted == {"credential": 1}
    staged = (tmp_path / "state" / "transcripts" / "-Users-a-x" / "s1.jsonl").read_text()
    assert "hunter2" not in staged and json.loads(staged)
    r = tp.push(tmp_path / "projects", send=False)
    assert r.scrubbed == 0 and r.unchanged == 1
