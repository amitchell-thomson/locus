# Bulk-ingest (pour) runbook

Operational checklist for pouring the multi-year corpus (build step 12 → BULK INGEST).
Everything re-ingest-bound is locked in (steps 1–12); this run is *additive* — pouring new
documents, not re-ingesting existing ones.

## Pre-flight

- [ ] **Laptop outbox**: the Mac outbox script's folder list is missing `coursework/`
      (confirmed not syncing 2026-06-06). Fix the folder list before staging, or the
      coursework slice of the corpus silently never arrives.
- [ ] `git status` clean; `uv run pytest -q` green.
- [ ] DB at head: `uv run python -c "from locus.config import load; from locus.db.migrate
      import migrate; migrate(load().paths.db)"` (or confirm `locus audit` runs without
      schema errors). Head as of step 12: revision 0008.
- [ ] **One ingest process** (the flock enforces it, but don't fight it): no other
      `locus ingest`/`locus sync`/`locus watch` running.
- [ ] **GPU idle and un-split**: check `nvidia-smi` — no leftover llama-server, no Ollama
      model in a KEPT CPU/GPU split. A split model produces identical output SLOWLY; no
      quality gate sees it (step 11.5 lesson). When in doubt:
      `ollama stop <model>` everything, let VRAM settle.
- [ ] Optional system deps present if the batch has slides / needs GPU vision:
      `soffice` (slide renders), `llama-server` (fast figure descriptions; falls back to
      Ollama with a gap line if absent).

## Staging

- Drop files into `vault/incoming/<category>/…` — the folder name (normalised singular)
  becomes `documents.category`: `papers/`, `coursework/`, `projects/`, `career/`, `notes/`.
- Repo snapshots: a directory under `incoming/projects/<name>/` is ONE repo unit
  (LocusDrop). Tracked repos need no staging — `locus sync` / `locus watch` handle them.
- Files must settle (the watcher waits for size-stability); partial syncs are safe.

## During the pour

Run `locus watch` (or batched `locus ingest <files>` under the flock) and monitor:

- [ ] **Quarantines stay at 0**: `vault/incoming/.quarantine/` — a quarantined doc is a
      bug to triage, not a casualty to accept. (Concurrent-ingest contention produces
      *spurious* quarantines — the flock prevents it; don't bypass it.)
- [ ] **GPU graph**: VRAM should cycle (text model ↔ VLM ↔ GOT) with full evictions
      between phases. GPU sitting idle while CPU is pinned across many cores = a split
      model or CPU vision encode — stop and fix before continuing (step 11.5/11.6).
- [ ] Ingest is unbounded-time by design (§2.4): the step-11 reference rate was 28 docs
      in ~5.2 h with figures + math OCR. Do not "speed it up".

## Post-pour validation (in order)

1. **`locus audit`** — gates:
   - QC zero (suspect props / noise entities / empty syntheses / corrupted fields);
   - gap liveness > 0 (zero corpus-wide = inert pass, loud warning fires);
   - **OCR-fallback pages in band** (heavy fallback = VRAM choreography regression —
     the step-11.5 failure mode; the counter warns above 20);
   - date/category distribution looks like the corpus you actually poured (a missing
     category = a staging/sync gap, e.g. the coursework outbox issue).
2. **`locus link`** — rebuild the alias substrate over the grown corpus:
   - verdict cache makes it incremental (only new/changed clusters hit the API);
   - review the audit's **suspicious merges** line (`locus audit`) and the oversize-skip
     log lines; consider `[alias].max_cluster_size` if many clusters were skipped;
   - **enable the related-docs stop-entity guard** at this scale: pass
     `stop_doc_freq ≈ 0.4 × doc_count` where `related_documents` is consumed (it is OFF
     by default — designed for the pour, see locus/link/related.py).
3. **`locus eval --suite full`** (+ the math suite is included) — gates vs the step-11/12
   baselines: recall@8 1.000, cross-domain 1.000, file_recall 1.000, banner rate 0.000,
   judge ≥ ~4.0 (n=8 noise band), math fidelity in the ~0.87–0.95 measured band,
   links_recall 1.000. Add labelled queries for the new corpus slices FIRST (eval labels
   live in `locus/eval/retrieval_eval.py`) — an eval that doesn't cover the new content
   validates nothing about it.
4. **Brute-force KNN ceiling (§11.D)**: check vector counts —
   `SELECT COUNT(*) FROM chunk_vectors` (+ propositions/sections/figures). sqlite-vec
   scans linearly; if retrieval latency degrades or counts approach ~10⁵, the ANN-index
   work item activates (post-pour list).
5. The MCP server picks up new aliases per query (no restart needed); restart it anyway
   if it has been running across the whole pour (stale-process lessons from round 3).

## After

- Update CLAUDE.md §2 (current state) with the pour result (doc count, eval numbers).
- Post-pour roadmap (not re-ingest-bound): ANN-index warning, Obsidian projection (§14),
  YouTube/podcast transcripts, broader retrieval tests.
