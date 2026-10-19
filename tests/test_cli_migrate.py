"""`locus migrate` — what `deploy/deploy.sh` runs after a backup when a pull brings a migration."""

from __future__ import annotations

import argparse
from pathlib import Path
from types import SimpleNamespace

from locus import cli
from locus.db.migrate import current_revision, head_revision


def test_migrate_brings_a_fresh_db_to_head_and_says_so(tmp_path: Path, monkeypatch, capsys) -> None:
    db = tmp_path / "fresh.db"
    monkeypatch.setattr(cli, "load", lambda: SimpleNamespace(paths=SimpleNamespace(db=db)))

    cli.cmd_migrate(argparse.Namespace())
    assert current_revision(db) == head_revision(db)
    assert "None ->" in capsys.readouterr().out

    cli.cmd_migrate(argparse.Namespace())
    assert "unchanged" in capsys.readouterr().out
