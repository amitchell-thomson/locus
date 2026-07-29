from locus import config as locus_config
from locus.db.connection import get_connection

cfg = locus_config.load()
conn = get_connection(cfg.paths.db)
for r in conn.execute(
    "SELECT id, source_uri, title, source_date, thesis FROM documents "
    "WHERE category='note' AND maturity='rough' ORDER BY id"
):
    print(f"[{r['id']}] {r['title'][:70]}")
    print(f"      uri:  {r['source_uri']}")
    print(f"      date: {r['source_date']}")
    print(f"      thesis: {(r['thesis'] or '')[:190]}")
    ents = [x["name"] for x in conn.execute(
        "SELECT DISTINCT name FROM entities WHERE doc_id=? AND type='concept' LIMIT 9", (r["id"],))]
    print(f"      concepts: {ents}")
    print()
