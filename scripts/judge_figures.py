"""Benchmark gate for the llama.cpp figure engine (plan step 11.6).

Samples ~10 stored figures (deterministic seed), generates fresh descriptions through a
live LlamaServer, and has Claude judge BOTH candidates against the actual figure image:
  - "ollama-stored": the description in the corpus (the fig-v2 backfill output — baseline)
  - "llamacpp": freshly generated, same weights, GPU vision encode

Adopt llamacpp iff: mean faithfulness >= baseline - 0.2 AND total hallucinated elements
<= baseline AND zero engine failures. Wall-clock s/figure recorded for both engines'
provenance (the llamacpp timing is measured here; the ollama baseline timing is the known
~27.5 s/fig from the backfill run).

Artifacts: eval-artifacts/figures/{results.json, report.md}.
"""

from __future__ import annotations

import json
import random
import sys
import time
from pathlib import Path

import anthropic

from locus.config import Config, load
from locus.db.connection import get_connection
from locus.eval.figure_fidelity import judge_description
from locus.ingest.figures import describe_figure
from locus.ingest.llamacpp import LlamaServer, LlamaServerError

SAMPLE = 10
SEED = 0
FAITHFULNESS_EPSILON = 0.2
OUT_DIR = Path("eval-artifacts/figures")

cfg = load()
conn = get_connection(cfg.paths.db)
rows = conn.execute(
    "SELECT f.id, f.doc_id, f.page, f.kind, f.caption, f.description, f.raw_path, d.title "
    "FROM figures f JOIN documents d ON d.id = f.doc_id "
    "WHERE f.description IS NOT NULL AND f.description != '' ORDER BY f.id"
).fetchall()
pool = [r for r in rows if (cfg.paths.raw_store / r["raw_path"]).exists()]
picked = pool if len(pool) <= SAMPLE else random.Random(SEED).sample(pool, SAMPLE)
# Spread check is by eye in the report (kind/doc columns); the seed makes it reproducible.
print(f"sample: {len(picked)} figures from a pool of {len(pool)}", flush=True)

judge = anthropic.Anthropic(api_key=Config.anthropic_api_key())
judge_model = cfg.generation.model

results = []
engine_failures = 0
t_llama_total = 0.0

try:
    with LlamaServer(cfg.figures) as server:
        for i, r in enumerate(picked, 1):
            png = (cfg.paths.raw_store / r["raw_path"]).read_bytes()
            t0 = time.time()
            fresh = describe_figure(png, r["caption"], client=server)
            dt = time.time() - t0
            t_llama_total += dt
            if fresh is None:
                engine_failures += 1
                print(f"[{i}/{len(picked)}] ENGINE FAILURE (QC/transport) fig={r['id']}", flush=True)
                continue
            j_stored = judge_description(judge, judge_model, png, r["description"])
            j_fresh = judge_description(judge, judge_model, png, fresh)
            results.append(
                {
                    "figure_id": r["id"], "doc": r["title"][:48], "page": r["page"],
                    "kind": r["kind"], "llamacpp_seconds": round(dt, 1),
                    "ollama": j_stored.model_dump(), "llamacpp": j_fresh.model_dump(),
                    "llamacpp_text": fresh,
                }
            )
            print(
                f"[{i}/{len(picked)}] fig={r['id']} {r['kind']:<6} {dt:5.1f}s | "
                f"faith ollama {j_stored.faithfulness} vs llama {j_fresh.faithfulness} | "
                f"halluc {j_stored.hallucinated_elements} vs {j_fresh.hallucinated_elements}",
                flush=True,
            )
except LlamaServerError as exc:
    print(f"FATAL: llama-server lifecycle failure: {exc}", flush=True)
    sys.exit(1)
finally:
    conn.close()

if not results:
    print("no judged pairs; cannot gate", flush=True)
    sys.exit(1)

n = len(results)
mean = lambda key, eng: sum(x[eng][key] for x in results) / n  # noqa: E731
o_faith, l_faith = mean("faithfulness", "ollama"), mean("faithfulness", "llamacpp")
o_conc, l_conc = mean("concreteness", "ollama"), mean("concreteness", "llamacpp")
o_hall = sum(x["ollama"]["hallucinated_elements"] for x in results)
l_hall = sum(x["llamacpp"]["hallucinated_elements"] for x in results)
s_per_fig = t_llama_total / len(picked)

gate = (
    l_faith >= o_faith - FAITHFULNESS_EPSILON and l_hall <= o_hall and engine_failures == 0
)

OUT_DIR.mkdir(parents=True, exist_ok=True)
(OUT_DIR / "results.json").write_text(json.dumps(results, indent=1))
report = f"""# llama.cpp figure-engine benchmark ({time.strftime('%Y-%m-%d')})

Sample: {n} judged figures (seed {SEED}), judge = {judge_model}

| metric | ollama (stored) | llamacpp (fresh) |
|---|---|---|
| mean faithfulness | {o_faith:.2f} | {l_faith:.2f} |
| mean concreteness | {o_conc:.2f} | {l_conc:.2f} |
| total hallucinated elements | {o_hall} | {l_hall} |
| engine failures | 0 | {engine_failures} |
| seconds/figure | ~27.5 (backfill measured) | {s_per_fig:.1f} |

Gate (faith >= baseline-{FAITHFULNESS_EPSILON}, halluc <=, zero failures): **{"PASS" if gate else "FAIL"}**
"""
(OUT_DIR / "report.md").write_text(report)
print(report, flush=True)
sys.exit(0 if gate else 2)
