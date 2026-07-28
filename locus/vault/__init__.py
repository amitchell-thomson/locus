"""Vault write layer (agent-layer plan §10) — how agents put text into the owner's notes.

The correctness linchpin of the capture layer: agents PROPOSE into clearly-owned, sentinel-marked
blocks and never touch the owner's prose (invariant 2), writes are atomic (never a half-mangled
note), and a two-writer conflict degrades to a sidecar rather than clobbering. Three small pieces:

  - `markers.py`  — the owned-block sentinels + find/replace of a block by kind.
  - `sidecar.py`  — sidecar path + Syncthing-conflict detection.
  - `writer.py`   — atomic upsert of an owned block; provenance for fully-generated notes.
"""
