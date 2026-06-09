# scripts/

One-off operational and benchmark scripts that live outside the package surface (`locus/`)
and the product CLI (`locus …`). They are kept as a record of how design decisions were made
and how the corpus was migrated — not part of the runtime. Run from the repo root, e.g.
`uv run python -m scripts.backfills.backfill_code_concepts`.

## `backfills/` — data migrations over the existing DB (no re-ingest)
Re-run a single ingest pass over already-stored rows and write the result back, so a new pass
doesn't require re-ingesting the whole corpus.

| script | purpose |
|---|---|
| `backfill_code_concepts.py` | code domain-concept entities from repo narrative (§1.2 Link) |
| `backfill_fig_descriptions.py` | re-describe figures under the fig-v2 prompt |
| `backfill_gaps.py` | recompute `documents.gap_flags` under the precision-filtered gap pass |
| `backfill_titles.py` | re-arbitrate `documents.title` via the synthesis pass |

## `benchmarks/` — the measurements that settled design decisions
Judged A/B and calibration runs on real corpus data. These are *why* the model/engine/threshold
choices in `config.toml` are what they are.

| script | purpose |
|---|---|
| `benchmark_mathocr.py` / `judge_mathocr.py` | race math-OCR engines, Claude-judge vs page image (chose GOT-OCR) |
| `judge_figures.py` | gate the llama.cpp GPU vision engine against the Ollama baseline |
| `calibrate_rerank_threshold.py` | fit `[retrieve].min_rerank_score` against in-corpus vs negative-control queries |
| `measure_sectioning.py` | extractor sectioning measurement (build step 4) |

## `reingest/` — historical one-off remediation drivers
Full re-ingests tied to specific past fixes; kept for the record, not for reuse.

| script | purpose |
|---|---|
| `reingest_recovery.py` | recovery re-ingest for the 2026-06-06 OOM finding |
| `reingest_round5.py` | round-5 remediation re-ingest |
| `reingest_step11.py` | step-11 figures re-ingest |

## `laptop-outbox/`
The Mac-side launchd agent + script that rsyncs dropped files to `vault/incoming/` on the
server (the ingest "outbox"). See its own `README.md`.
