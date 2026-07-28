"""Capture loops (agent-layer plan §8) — turning the owner's capture into corpus.

Loop A (handwriting, §8.1): the reMarkable device renders each changed notebook and pushes the
PDF to the server (transport built in Phase 0). This package does the server-side rest:
  - `remarkable.py` — identify a staged `<uuid>.pdf`: map it to its reMarkable name + folder
    (via rmapi) and a Locus category.
  - (next) transcribe.py · fillin.py — vision transcription + conservative gap fill.
"""
