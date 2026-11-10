"""heuris.db and the indexer (H1.3): incremental by hash, prune only on request, filters in KNN."""

from __future__ import annotations

import hashlib
import struct
from datetime import datetime, timezone

import pytest

from locus.heuris import db as hdb
from locus.heuris.index import Item, chunk_text, index, record_run


def fake_embed(texts):
    out = []
    for t in texts:
        h = hashlib.sha256(t.encode()).digest()
        v = [((h[i % 32] / 255.0) - 0.5) for i in range(768)]
        n = sum(x * x for x in v) ** 0.5
        out.append([x / n for x in v])
    return out


@pytest.fixture
def conn(tmp_path):
    c = hdb.connect(tmp_path / "heuris.db")
    yield c
    c.close()


def _item(ref, text="hello", thing=None, kind="mail", day=1, people=()):
    return Item(ref=ref, kind=kind, at=datetime(2026, 11, day, 9, tzinfo=timezone.utc), title=f"t {ref}",
                text=text, thing=thing, people=list(people))


def test_second_run_over_unchanged_data_adds_nothing(conn):
    calls = []

    def counting(texts):
        calls.append(len(texts))
        return fake_embed(texts)

    items = [_item(f"mail:a/{i}") for i in range(5)]
    st = index(conn, "mail", items, embed=counting)
    assert (st.added, st.unchanged) == (5, 0)
    st = index(conn, "mail", items, embed=counting)
    assert (st.added, st.unchanged) == (0, 5)
    assert calls == [5]  # the second run embedded nothing


def test_a_changed_item_replaces_its_chunks(conn):
    index(conn, "mail", [_item("mail:a/1", "one")], embed=fake_embed)
    index(conn, "mail", [_item("mail:a/1", "two " * 1000)], embed=fake_embed)
    n_items = conn.execute("SELECT COUNT(*) FROM items").fetchone()[0]
    n_chunks = conn.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
    n_fts = conn.execute("SELECT COUNT(*) FROM chunk_fts").fetchone()[0]
    n_vec = conn.execute("SELECT COUNT(*) FROM chunk_vec").fetchone()[0]
    assert n_items == 1 and n_chunks == n_fts == n_vec > 1
    assert not conn.execute("SELECT 1 FROM chunk_fts WHERE chunk_fts MATCH 'one'").fetchone()


def test_prune_only_when_asked(conn):
    index(conn, "mail", [_item("mail:a/1"), _item("mail:a/2")], embed=fake_embed)
    st = index(conn, "mail", [_item("mail:a/1")], embed=fake_embed)
    assert st.removed == 0 and conn.execute("SELECT COUNT(*) FROM items").fetchone()[0] == 2
    st = index(conn, "mail", [_item("mail:a/1")], embed=fake_embed, prune=True)
    assert st.removed == 1 and conn.execute("SELECT COUNT(*) FROM items").fetchone()[0] == 1
    assert conn.execute("SELECT COUNT(*) FROM chunk_vec").fetchone()[0] == 1


def test_prune_never_touches_another_source(conn):
    index(conn, "mail", [_item("mail:a/1")], embed=fake_embed)
    index(conn, "chats", [_item("chat:s#1", kind="chat")], embed=fake_embed, prune=True)
    assert conn.execute("SELECT COUNT(*) FROM items").fetchone()[0] == 2


def test_knn_applies_filters_before_ranking(conn):
    # 30 items about nothing in particular, one tagged quast; a filtered KNN with k=3 must find it.
    items = [_item(f"mail:a/{i}", f"noise {i}") for i in range(30)]
    items.append(_item("commit:quast@abc", "barrier sweep", thing="quast", kind="commit"))
    index(conn, "mail", items, embed=fake_embed)
    q = fake_embed(["anything"])[0]
    rows = conn.execute(
        "SELECT chunk_id FROM chunk_vec WHERE embedding MATCH ? AND k = 3 AND thing = 'quast'",
        (struct.pack("768f", *q),),
    ).fetchall()
    assert len(rows) == 1


def test_people_are_stored_lowercased(conn):
    index(conn, "mail", [_item("mail:a/1", people=[("Sam.Smith@X.com", "from"), ("Sam Smith", "name")])],
          embed=fake_embed)
    got = {r[0] for r in conn.execute("SELECT person FROM item_people")}
    assert got == {"sam.smith@x.com", "sam smith"}


def test_record_run_keeps_last_ok_through_a_failure(conn):
    index(conn, "mail", [_item("mail:a/1")], embed=fake_embed)
    record_run(conn, "mail", None)
    ok = conn.execute("SELECT last_ok FROM sources WHERE name='mail'").fetchone()[0]
    record_run(conn, "mail", None, error="rsync failed")
    row = conn.execute("SELECT last_ok, error, items FROM sources WHERE name='mail'").fetchone()
    assert row[0] == ok and row[1] == "rsync failed" and row[2] == 1


def test_chunking_is_bounded_and_covers_the_text():
    text = "\n\n".join(f"paragraph {i} " + "word " * 80 for i in range(20))
    chunks = chunk_text(text)
    assert all(len(c) <= 1800 for c in chunks)
    assert chunks[0].startswith("paragraph 0") and "paragraph 19" in chunks[-1]
    assert chunk_text("short") == ["short"]


def test_a_newer_schema_refuses_and_says_rebuild(tmp_path):
    p = tmp_path / "h.db"
    c = hdb.connect(p)
    c.execute("PRAGMA user_version=99")
    c.close()
    with pytest.raises(RuntimeError, match="rebuild"):
        hdb.connect(p)
