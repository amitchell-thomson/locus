"""Cross-corpus linking layer (plan step 12): runs AFTER ingest, over the assembled corpus.

`aliases` builds the entity-alias substrate (canonical names across documents);
`related` exposes joins-only document↔document connections over it. Parallel to
`retrieve/` — nothing here is in the ingest path, and everything is derived + regenerable.
"""
