# Screens v2 (18 Oct 2026)

`screens-v2.pdf` is the reference: the document pushed to the tablet as "2026-10-18 Locus
screens v2" and marked up in design round 1 (taken from the device bundle 19 Oct, without ink).
Plan v4 amends it (Nerd Font glyphs, footer, Explore's `[e]`, careers in the projects list).

`screens.tex` and `mock.py` are its sources, recovered from `.v3work/`. **`mock.py` no longer
reproduces the PDF**: it was edited after the render, and it now fails its own width checks
(the Home split overflows by three columns, and `Box.top` with a right label is one short).
Read the PDF, not the script.
