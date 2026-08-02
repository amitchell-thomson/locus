"""Render a representative four-page daily page to `eval-artifacts/daily-page-sample.pdf`.

Kept because LAYOUT CANNOT BE UNIT-TESTED. The tests assert the page-break count and that no
section overflows, but whether the writing lines are far enough apart to write between, and
whether a page looks composed rather than crammed at the top, can only be seen. This seeds a tmp
DB with one item of each kind and renders the real PDF through the real toolchain.

    uv run python scripts/analysis/render_daily_sample.py
"""
import tempfile
from datetime import date
from pathlib import Path

from locus.agent import compose_daily as cd
from locus.agent import state
from locus.db.connection import get_connection
from locus.db.migrate import migrate
from locus.learn import review as lr
from locus.reading.md2pdf import PageGeometry, render_markdown_to_pdf

tmp = Path(tempfile.mkdtemp())
db = tmp / "d.db"
migrate(db)
c = get_connection(db)

with c:
    c.execute("INSERT INTO reading_proposals (kind,dedupe_key,title,why,why_kind,evidence_key,"
              "status,score,proposed_at,created_at,why_long,why_written_at) VALUES "
              "('paper','k1','Honey, I Shrunk the Covariance Matrix','cited by 2 papers you kept',"
              "'citation','papers/regime.pdf','proposed',2.0,'2026-07-12','2026-07-12',"
              "'regime-ml estimates a 55x55 covariance from 250 days of returns "
              "(features/factor_cov.py:88). Ledoit-Wolf shrinkage is the standard fix at that "
              "ratio, and you have never used it.','2026-07-26')")
    c.execute("INSERT INTO reading_proposals (kind,dedupe_key,title,why,why_kind,evidence_key,"
              "status,score,proposed_at,created_at,device_folder) VALUES "
              "('paper','k2','Backtesting Value-at-Risk','cited by a paper you kept','citation',"
              "'papers/var.pdf','accepted',1.0,'2026-07-20','2026-07-20','In-Progress')")
    c.execute("INSERT INTO pdf_annotations (source_uri,pdf_page,kind,bbox_key,covered_text,"
              "in_margin,captured_at,note) VALUES ('books/apm.pdf',113,'underline','b1',"
              "'the extreme case in which every hedge fund holds a copy of the same portfolio',"
              "0,'2026-07-30T10:00:00','is a factor like a feature in ML?')")
    c.execute("INSERT INTO documents (content_hash,source_type,source_uri,raw_path,ingest_model,"
              "title) VALUES ('h','pdf','papers/x.pdf','raw/x.pdf','test','A paper')")
    c.execute("INSERT INTO sections (doc_id,position,title) VALUES (1,0,'Intro')")
    c.execute("INSERT INTO propositions (doc_id,section_id,position,text,embed_model) VALUES "
              "(1,1,0,'A factor covariance estimator decomposes returns r into Bf + e, where B "
              "holds the factor loadings estimated from the return history.','nomic')")

oid, _ = state.upsert_object(c, type_="question", title="is a factor like a feature in ML?")
state.set_status(c, oid, "active")
state.apply_owner_edit(c, oid, {"development": [{"at": "2026-07-28", "text":
    "features are learned by optimisation; loadings are estimated from returns. not the same."}]},
    source="daily:2026-07-28#T1")

item = lr.schedule_prompt(c, prompt_kind="proposition", prompt_ref="1", today=date(2026, 1, 1))
lr.set_question(c, item.id, "What does a factor covariance estimator decompose returns into, "
                            "and where do the loadings come from?")

page = cd.compose(c, today=date(2026, 8, 2))
md = cd.render(page)
out = Path("eval-artifacts/daily-page-sample.pdf")
out.parent.mkdir(exist_ok=True)
render_markdown_to_pdf(md, out, geometry=PageGeometry(rule_gap_em=2.6))
print(md)
print("\n=== wrote", out, "===")
c.close()
