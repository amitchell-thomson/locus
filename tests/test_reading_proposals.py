"""Phase-4 accept loop: migration 0017, proposal lifecycle, and the device folder watch.

Model-free and network-free — a seeded tmp DB plus a fake `rmapi` runner. What is asserted here is
the set of rules the loop would be worthless without, not the CRUD:

  - grounded-or-silent (a proposal with no why is refused);
  - propose-never-mutate (a stub never becomes a corpus document);
  - the stock cap (a full folder proposes nothing);
  - the dedupe key folds the OCR manglings the corpus actually contains;
  - a failed or empty device listing changes NOTHING — the mass-rejection guard.
"""

from __future__ import annotations

import sqlite3
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.reading import proposals as P
from locus.reading import watch as W


@pytest.fixture()
def conn(tmp_path: Path):
    db = tmp_path / "reading.db"
    migrate(db)
    c = get_connection(db)
    yield c
    c.close()


def _seed(conn, **kw) -> int:
    args = dict(
        kind="paper", title="A Paper", why="cited by your regime work",
        why_kind="citation", evidence_key="vault/incoming/papers/x.pdf",
    )
    args.update(kw)
    new_id = P.add_candidate(conn, **args)
    assert new_id is not None
    return new_id


def _runner(paths: list[str], *, code: int = 0):
    """Fake rmapi: `find` returns `[f] ` lines, `stat` returns an id."""
    def run(args):
        if args[0] == "find":
            return code, "".join(f"[f] {p}\n" for p in paths), ""
        if args[0] == "stat":
            return 0, '{"ID": "uuid-1", "Name": "x"}', ""
        return 0, "", ""
    return run


# ---------- migration ----------


def test_0017_tables_exist(conn):
    names = {r["name"] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    assert "reading_proposals" in names
    assert "reading_targets" in names


def test_discovery_surface_is_allowed_and_reading_still_is(conn):
    # 0017 widens the CHECK; the daily page's own 'reading' surface must survive untouched.
    for surface in ("discovery", "reading"):
        conn.execute(
            "INSERT INTO acceptance_log (surface, candidate_key, verdict, at) VALUES (?,?,?,?)",
            (surface, "k", "kept", "2026-07-31"),
        )
    with pytest.raises(sqlite3.IntegrityError):
        conn.execute(
            "INSERT INTO acceptance_log (surface, candidate_key, verdict, at) VALUES (?,?,?,?)",
            ("nonsense", "k", "kept", "2026-07-31"),
        )


def test_spine_untouched_by_0017(conn):
    cols = {r["name"] for r in conn.execute("PRAGMA table_info(documents)")}
    assert "why" not in cols and "status" not in cols


# ---------- identity ----------


def test_dedupe_key_folds_the_ocr_manglings_in_the_corpus():
    # Both surfaces of ONE work are present in the live corpus today (entity pass, doc 140).
    loose = P.dedupe_key("L¨utkepohl and Wo´zniak, 2020")
    real = P.dedupe_key("Lütkepohl and Woźniak, 2020")
    assert loose == real == "lutkepohl and wozniak 2020"


def test_dedupe_key_ignores_case_and_punctuation():
    assert P.dedupe_key("Deep Hedging: A Review!") == P.dedupe_key("deep hedging   a review")


def test_a_known_work_is_not_proposed_twice(conn):
    first = _seed(conn, title="Shrinkage Estimation of Covariance")
    again = P.add_candidate(
        conn, kind="paper", title="shrinkage estimation of covariance!", why="w",
        why_kind="gap", evidence_key="e",
    )
    assert first is not None and again is None


def test_a_rejected_work_is_never_re_proposed(conn):
    pid = _seed(conn, title="Old News")
    P.set_status(conn, pid, "rejected", resolution="ttl")
    assert P.add_candidate(
        conn, kind="paper", title="Old News", why="w", why_kind="gap", evidence_key="e",
    ) is None


# ---------- grounded or silent ----------


@pytest.mark.parametrize("why,evidence", [("", "e"), ("   ", "e"), ("w", ""), ("w", "  ")])
def test_an_ungrounded_proposal_is_refused(conn, why, evidence):
    with pytest.raises(ValueError):
        P.add_candidate(
            conn, kind="paper", title="T", why=why, why_kind="gap", evidence_key=evidence,
        )


# ---------- the stock cap ----------


def test_cap_is_on_the_stock_not_the_flow(conn):
    """The invariant, not the number: the cap counts what is SITTING in Proposed.

    Asserted against `DEFAULT_CAPS` rather than a literal, so retuning the cap (3 -> 10 on
    2026-07-31) does not require editing a test whose subject is the mechanism.
    """
    cap = P.DEFAULT_CAPS["paper"]
    assert P.slots_free(conn, "paper") == cap
    for i in range(cap):
        pid = _seed(conn, title=f"Paper {i}")
        P.mark_proposed(conn, pid, filename=f"2026-07-31 Paper {i}.pdf")
    assert P.slots_free(conn, "paper") == 0, "a full folder must propose nothing"

    # Resolving one frees exactly one slot — the stock fell, no quota was refilled.
    held = P.list_proposals(conn, status="proposed")
    P.set_status(conn, held[0].id, "accepted", resolution="moved")
    assert P.slots_free(conn, "paper") == 1


def test_books_get_a_single_slot(conn):
    assert P.slots_free(conn, "book") == 1
    pid = _seed(conn, kind="book", title="A Book")
    P.mark_proposed(conn, pid, filename="2026-07-31 A Book.pdf")
    assert P.slots_free(conn, "book") == 0


# ---------- the accept signal ----------


def test_moving_out_of_proposed_accepts(conn):
    pid = _seed(conn, title="Regime Shifts")
    P.mark_proposed(conn, pid, filename="2026-07-31 Regime Shifts.pdf")

    out = W.scan(conn, runner=_runner(["Reading/In-Progress/2026-07-31 Regime Shifts"]))
    assert [(o.action, o.resolution) for o in out] == [("accepted", "moved")]
    assert P.list_proposals(conn, status="accepted")[0].id == pid


def test_staying_in_proposed_holds_until_the_ttl(conn):
    pid = _seed(conn, title="Slow Burn")
    P.mark_proposed(conn, pid, filename="2026-07-31 Slow Burn.pdf")
    runner = _runner(["Reading/Proposed/2026-07-31 Slow Burn"])

    assert W.scan(conn, runner=runner)[0].action == "held"

    later = datetime.now(timezone.utc) + timedelta(days=22)
    out = W.scan(conn, runner=runner, now=later)
    assert (out[0].action, out[0].resolution) == ("rejected", "ttl")


def test_deleting_it_is_a_stronger_no_than_leaving_it(conn):
    pid = _seed(conn, title="Gone")
    P.mark_proposed(conn, pid, filename="2026-07-31 Gone.pdf")
    # It is not under the reading root any more, but the listing is healthy.
    out = W.scan(conn, runner=_runner(["Reading/Proposed/2026-07-31 Something Else"]))
    assert (out[0].action, out[0].resolution) == ("rejected", "removed")


def test_an_empty_listing_rejects_nothing(conn):
    """A transient rmapi failure must never look like 'he deleted all of them'."""
    pid = _seed(conn, title="Safe")
    P.mark_proposed(conn, pid, filename="2026-07-31 Safe.pdf")

    assert W.scan(conn, runner=_runner([])) == []
    assert P.list_proposals(conn, status="proposed")[0].id == pid


def test_a_failed_listing_raises_rather_than_rejecting(conn):
    pid = _seed(conn, title="Safe")
    P.mark_proposed(conn, pid, filename="2026-07-31 Safe.pdf")
    with pytest.raises(RuntimeError):
        W.scan(conn, runner=_runner(["ignored"], code=1))
    assert P.list_proposals(conn, status="proposed")[0].id == pid


def test_acceptance_is_logged_to_the_discovery_surface(conn):
    pid = _seed(conn, title="Logged")
    P.mark_proposed(conn, pid, filename="2026-07-31 Logged.pdf")
    W.scan(conn, runner=_runner(["Reading/Finished/2026-07-31 Logged"]))

    rows = conn.execute(
        "SELECT surface, verdict FROM acceptance_log WHERE surface='discovery'"
    ).fetchall()
    assert [(r["surface"], r["verdict"]) for r in rows] == [("discovery", "kept")]


# ---------- stubs never enter the corpus ----------


def test_a_stub_is_never_ingested(conn, tmp_path):
    from locus.reading.accept import ingest_accepted

    pid = _seed(conn, kind="book", title="A Book We Do Not Have")
    P.mark_proposed(conn, pid, filename="2026-07-31 A Book.pdf")  # no local_path => stub
    P.set_status(conn, pid, "accepted", resolution="moved")

    results = ingest_accepted(conn, incoming=tmp_path)
    assert [r.status for r in results] == ["awaiting-file"]
    assert P.list_proposals(conn, status="accepted")[0].id == pid, "must stay accepted, not ingest"


# ---------- the annotation join key ----------


def test_marks_reach_the_document_only_through_reading_targets(conn):
    """The whole point of 0017: device-path annotations join to a filesystem-path document."""
    conn.execute(
        "INSERT INTO pdf_annotations (source_uri, doc_uuid, pdf_page, kind, bbox_key, "
        "captured_at) VALUES (?,?,?,?,?,?)",
        ("/reading_list/Advanced Portfolio Management", "uuid-book", 71, "underline", "1,2,3,4",
         "2026-07-31"),
    )
    conn.commit()
    assert P.annotated_source_uris(conn) == {}, "unmapped marks reach nothing"

    P.link_target(
        conn, source_uri="vault/incoming/paper/Advanced Portfolio Management.pdf",
        doc_uuid="uuid-book", linked_by="manual",
    )
    assert P.annotated_source_uris(conn) == {
        "vault/incoming/paper/Advanced Portfolio Management.pdf": 1
    }


# ---------- delivery ----------


def test_filename_is_date_prefixed_and_sanitised():
    from datetime import date

    from locus.reading.deliver import safe_filename

    # rmapi REFUSES a same-named re-upload, so a stable name breaks every run after the first.
    name = safe_filename("Regime: Shifts/Detection?", on=date(2026, 7, 31))
    assert name.startswith("2026-07-31 ")
    assert not set(name) & set('/\\:*?"<>|')
    assert name.endswith(".pdf")


def _recording_runner(calls: list):
    def run(args):
        calls.append(args)
        if args[0] == "stat":
            return 0, '{"ID": "uuid-xyz"}', ""
        return 0, "", ""
    return run


def test_delivery_creates_the_three_folders_and_records_the_uuid(conn, tmp_path):
    from datetime import date

    from locus.reading.deliver import deliver_proposal

    pdf = tmp_path / "paper.pdf"
    pdf.write_bytes(b"%PDF-1.4 fake")
    pid = _seed(conn, title="Regime Shifts")
    prop = P.list_proposals(conn, status="candidate")[0]

    calls: list = []
    out = deliver_proposal(
        conn, prop, pdf, runner=_recording_runner(calls), on=date(2026, 7, 31), is_real=True,
    )

    mkdirs = {c[1] for c in calls if c[0] == "mkdir"}
    assert {"Reading/Proposed", "Reading/In-Progress",
            "Reading/Finished"} <= mkdirs, "the move target must exist before he moves it"
    assert out.device_uuid == "uuid-xyz"

    stored = P.list_proposals(conn, status="proposed")[0]
    assert stored.id == pid and stored.filename == "2026-07-31 Regime Shifts.pdf"
    assert stored.local_path == str(pdf) and not stored.is_stub


def test_a_delivered_stub_records_no_local_path(conn, tmp_path):
    """A stub describes the work; ingesting it would put our own words in the corpus."""
    from locus.reading.deliver import deliver_proposal

    pdf = tmp_path / "stub.pdf"
    pdf.write_bytes(b"%PDF-1.4 stub")
    prop_id = _seed(conn, kind="book", title="Some Book")
    prop = P.list_proposals(conn, status="candidate")[0]

    deliver_proposal(conn, prop, pdf, runner=_recording_runner([]), is_real=False)

    stored = next(p for p in P.list_proposals(conn, status="proposed") if p.id == prop_id)
    assert stored.local_path is None and stored.is_stub


def test_channel_stats_separate_ttl_from_removal(conn):
    """`ttl` and `removed` are both rejections and must not be pooled: one means he left it
    sitting, the other that he threw it away.

    Grouped by `evidence_key`, not `why_kind`. `why_kind` is `'discovery'` on every row the
    pipeline has ever produced, so grouping by it yielded a single bucket and the per-channel
    breakdown this function exists for could never appear however much evidence accumulated.
    """
    a = _seed(conn, title="A", evidence_key="project:regime-ml")
    b = _seed(conn, title="B", evidence_key="project:regime-ml")
    c = _seed(conn, title="C", evidence_key="project:regime-ml")
    P.set_status(conn, a, "accepted", resolution="moved")
    P.set_status(conn, b, "rejected", resolution="ttl")
    P.set_status(conn, c, "rejected", resolution="removed")

    stats = P.channel_stats(conn)["project:regime-ml"]
    assert (stats["kept"], stats["ttl"], stats["removed"]) == (1, 1, 1)


def test_channel_stats_distinguishes_subjects(conn):
    """The point of the regrouping: two subjects must be two buckets, which the old key
    (constant `why_kind`) could never produce."""
    keep = _seed(conn, title="K", evidence_key="project:regime-ml")
    drop = _seed(conn, title="D", evidence_key="project:python-solutions")
    P.set_status(conn, keep, "accepted", resolution="moved")
    P.set_status(conn, drop, "rejected", resolution="removed")

    stats = P.channel_stats(conn)
    assert stats["project:regime-ml"]["kept"] == 1
    assert stats["project:python-solutions"]["removed"] == 1


def test_every_rmapi_path_rendering_parses(conn):
    """rmapi renders paths relative to the PARENT of what it searched, and without a leading slash.

    `rmapi find /Locus/Reading` returns `Reading/Proposed/<name>`; `rmapi find /Locus` returns
    `Locus/Reading/Proposed/<name>`. Prefix-matching the root matched NONE of them, so ten papers
    sat on the device while the watch saw zero — and the scan would have scored that as deletion.
    Verified against the live device, not assumed.
    """
    pid = _seed(conn, title="Both Forms")
    P.mark_proposed(conn, pid, filename="2026-07-31 Both Forms.pdf")

    # All three renderings rmapi is known to emit, depending on what it was asked to search.
    for path in ("Reading/Finished/2026-07-31 Both Forms",
                 "Locus/Reading/Finished/2026-07-31 Both Forms",
                 "/Locus/Reading/Finished/2026-07-31 Both Forms"):
        entries = W.list_reading_entries(_runner([path]))
        assert [e.folder for e in entries] == ["Finished"], path


def test_device_entries_carry_an_absolute_path_usable_by_rmapi_get(conn):
    """`rmapi find` renders paths relative to the parent of what it searched.

    Feeding that back to `rmapi get` fails, which is how the first real annotation sweep died —
    on a paper the owner had genuinely moved to In-Progress and written on.
    """
    entries = W.list_reading_entries(
        _runner(["Reading/In-Progress/2026-07-31 AlphaZeroBeta"]), root="/Locus/Reading"
    )
    assert entries[0].path == "/Locus/Reading/In-Progress/2026-07-31 AlphaZeroBeta"
    assert entries[0].folder == "In-Progress"


def test_rmdoc_fetch_closes_stdin_and_honours_a_short_timeout(monkeypatch, tmp_path):
    """An hourly job cannot answer a re-auth prompt, and it holds the ingest lock while it waits.

    Measured 2026-08-01: one fetch blocked in `do_poll` for half an hour on the 1800s default,
    stalling the whole reading pipeline and every other ingest with it.
    """
    import subprocess

    from locus.capture import rmdoc

    seen = {}

    def fake_run(cmd, **kw):
        seen.update(kw)
        raise subprocess.TimeoutExpired(cmd, kw.get("timeout", 0))

    monkeypatch.setattr(rmdoc.__dict__.get("subprocess", subprocess), "run", fake_run,
                        raising=False)
    monkeypatch.setattr(subprocess, "run", fake_run)
    with pytest.raises(subprocess.TimeoutExpired):
        rmdoc.fetch_rmdoc("/Locus/Reading/In-Progress/X", tmp_path, timeout=5)
    assert seen["timeout"] == 5, "the caller's timeout must win over the interactive default"
    assert seen["stdin"] == subprocess.DEVNULL, "a scheduled job must never wait on a prompt"
