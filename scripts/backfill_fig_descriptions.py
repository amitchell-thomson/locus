"""Backfill fig-v1 figure descriptions to the fig-v2 prompt (2026-06-06 audit, finding 2).

Descriptions are regenerable from the stored raw-store PNGs (a deliberate step-11 design
property), so this touches ONLY figures.description + figure_vectors — no re-ingest, no
text passes, no OCR. v1/v2 detection is exact: a figure is already v2 iff its image's
v2-keyed pass_cache entry exists and matches the stored description; everything else is
re-described under the current prompt and re-embedded.

A v2 description that fails QC twice keeps the stored v1 text (never downgrade data).
This run doubles as the validation for the text->VLM handoff settle guard: healthy is
~10-15 s/figure with the VLM 100% on GPU (printed after the first call).
"""

from __future__ import annotations

import logging
import sys
import time

from locus.config import load
from locus.db.connection import get_connection
from locus.ingest import embed, llm
from locus.ingest.figures import describe_figure, index_text
from locus.ingest_lock import IngestLockHeld, ingest_lock
from locus.ingest_pipeline import _FigureCache, _vec_blob

logging.basicConfig(level=logging.WARNING)

cfg = load()
conn = get_connection(cfg.paths.db)
cache = _FigureCache(conn)

# Effective model = the engine that will produce the descriptions (cache keys are
# engine-separated, step 11.6).
fig_cfg = cfg.figures
effective_model = fig_cfg.llamacpp_model if fig_cfg.engine == "llamacpp" else fig_cfg.model

todo: list[tuple[int, int, str, str | None, bytes]] = []  # (fig_id, doc_id, raw_path, caption, png)
already = missing = 0
for r in conn.execute("SELECT id, doc_id, raw_path, caption, description FROM figures ORDER BY doc_id, position"):
    path = cfg.paths.raw_store / r["raw_path"]
    try:
        png = path.read_bytes()
    except OSError:
        missing += 1
        continue
    cached = cache.get(png, effective_model)  # keyed: engine model + PROMPT_VERSION
    if cached is not None and (cached or None) == (r["description"] or None):
        already += 1
        continue
    todo.append((r["id"], r["doc_id"], r["raw_path"], r["caption"], png))

print(f"figures: {already} already v2, {len(todo)} to backfill, {missing} missing PNGs", flush=True)
if not todo:
    sys.exit(0)

t_start = time.time()
done = kept_v1 = 0


def _engine_client():
    """One llama-server for the WHOLE run when engine='llamacpp' (no interleaved text
    passes here, unlike per-doc ingest); None = the Ollama path."""
    if fig_cfg.engine != "llamacpp":
        llm.unload_all()  # start the VLM on a clean card (and exercise the settle guard)
        from contextlib import nullcontext

        return nullcontext(None)
    from locus.ingest.llamacpp import LlamaServer

    return LlamaServer(fig_cfg)


try:
    with ingest_lock(), _engine_client() as vlm_client:
        for i, (fig_id, doc_id, raw_path, caption, png) in enumerate(todo, 1):
            t0 = time.time()
            desc = describe_figure(png, caption, client=vlm_client)
            if i == 1:  # guard-4 validation evidence: is the VLM split?
                try:
                    from ollama import Client

                    for m in Client(host=cfg.ollama.host).ps().models or []:
                        pct = 100 * (m.size_vram or 0) / (m.size or 1)
                        print(f"VLM residency check: {m.model} {pct:.0f}% on GPU", flush=True)
                except Exception:
                    pass
            if desc is None:
                kept_v1 += 1  # QC failed twice: keep the stored v1 description
                continue
            cache.stage(png, desc, effective_model)
            text = index_text(caption, desc)
            vec = embed.embed_text(text) if text else None
            with conn:
                conn.execute(
                    "UPDATE figures SET description=?, embed_model=? WHERE id=?",
                    (desc, embed.embedding_model(), fig_id),
                )
                conn.execute("DELETE FROM figure_vectors WHERE figure_id=?", (fig_id,))
                if vec is not None:
                    conn.execute(
                        "INSERT INTO figure_vectors (figure_id, embedding) VALUES (?,?)",
                        (fig_id, _vec_blob(vec)),
                    )
            done += 1
            if i % 20 == 0 or i == len(todo):
                rate = (time.time() - t_start) / i
                print(f"[{i}/{len(todo)}] doc={doc_id} {rate:.1f}s/fig avg", flush=True)
            _ = t0
        cache.flush()
except IngestLockHeld as exc:
    print(exc, flush=True)
    sys.exit(1)
finally:
    conn.close()

print(
    f"DONE in {(time.time() - t_start) / 60:.1f} min: {done} re-described, "
    f"{kept_v1} kept v1 (v2 QC fail), {missing} missing PNGs",
    flush=True,
)
