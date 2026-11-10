"""heuris: one index over mail, calendar, chats and repositories, searched with the library (H1).

The spec is docs/redesign/slices/h1-index-and-tools.md; the design is docs/redesign/heuris.tex.
`heuris.db` holds the new kinds (D1); sources and ink stay in `locus.db` and are searched where
they are, then reranked with everything else as one list.
"""
