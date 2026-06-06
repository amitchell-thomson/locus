"""Figure image loading for generation surfaces (step 11, tier 3 — §15.1).

Shared by query.py (base64 image blocks in the Claude call) and mcp_server.py (MCP image
content). Loads a retrieved figure's PNG from the flat raw store and bounds its size:
Claude's universal image ceiling is 1568px on the long edge (older models; newer accept
more — we target the bound that holds for any configured generation model), and oversized
payloads waste image tokens for no retrieval gain. Missing or unreadable files degrade to
None — the figure's TEXT (caption+description) is already in the assembled context, so a
lost image never errors a query.
"""

from __future__ import annotations

import logging

from locus.config import load

log = logging.getLogger(__name__)

MAX_EDGE_PX = 1568  # Claude's recommended max long edge (universal across models)
MAX_BYTES = 3 * 1024 * 1024  # downscale anything heavier; API hard limits are above this


def load_figure_png(raw_path: str) -> bytes | None:
    """PNG bytes for a figure's raw-store file, downscaled when oversized. None on failure."""
    path = load().paths.raw_store / raw_path
    try:
        data = path.read_bytes()
    except OSError as exc:
        log.warning("figure image unavailable (%s): %s", raw_path, exc)
        return None
    return _bounded(data, raw_path)


def _bounded(data: bytes, name: str) -> bytes | None:
    """Downscale a PNG that exceeds the edge/byte bounds; pass small ones through."""
    try:
        from io import BytesIO

        from PIL import Image

        with Image.open(BytesIO(data)) as im:
            w, h = im.size
            if max(w, h) <= MAX_EDGE_PX and len(data) <= MAX_BYTES:
                return data
            scale = MAX_EDGE_PX / max(w, h)
            if scale < 1.0:
                im = im.resize((max(1, round(w * scale)), max(1, round(h * scale))))
            out = BytesIO()
            im.save(out, format="PNG", optimize=True)
            return out.getvalue()
    except Exception as exc:  # a corrupt PNG must not error the query
        log.warning("figure image downscale failed (%s): %s", name, exc)
        return None
