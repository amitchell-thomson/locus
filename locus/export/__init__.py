"""Read-only projections of the corpus (render targets, never in the ingest/retrieval path).

Currently: the Obsidian vault projection (§13). One-way, regenerable, joins-only.
"""

from locus.export.obsidian import ExportReport, export_vault

__all__ = ["ExportReport", "export_vault"]
