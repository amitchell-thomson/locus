"""Grounded enrichment of capture notes (agent-layer plan §8.1).

After a note is transcribed + filled, `related.py` retrieves the corpus units it connects to
(in-process, no API — grounding is deterministic and free) and writes a `> [!ai] Related` owned
block into the note via the vault writer. Grounded-or-silent (invariant 3): a link appears only if
it cites a real retrieved document; a low-confidence retrieval writes no block.
"""
