---
name: remarkable-explainer
description: "Write a document for Alec to read on paper on his reMarkable, usually to understand something. Use whenever he asks for an explanation, description, primer, brief, plan or write-up to be sent to the tablet, or says things like \"explain X and put it on my remarkable\", \"send me a description of this\", \"write this up for paper\", \"I want to understand X\". Covers the format contract (LaTeX, two columns), the house voice (no assistant register, no em dashes), and how to build an explanation that actually lands."
---

# Writing for the reMarkable

He reads these on paper, away from a screen, usually because he is trying to **understand
something**. Occasionally it is a plan or a brief to think over. Either way the document has one
job: leave him understanding a thing he did not understand before, without needing you there.

Send with the Locus MCP tool: `to_remarkable(latex=..., title=...)`. Author in LaTeX, pass a
fragment starting at body text, and give the name as `title` rather than writing your own opening
heading. If the tool is unavailable, say so plainly rather than sending markdown instead.

## How to build the explanation

This is the part that decides whether the document works.

**Start from the object, not the vocabulary.** Open with the physical or concrete thing and let
the terminology arrive when it is needed to say something. "A chip with a sheet of electrons
under the surface and metal gates on top" earns the word *quantum dot* two sentences later. A
document that opens by defining five terms has spent its first page before it said anything.

**Define at first use, then use it.** Every technical word gets one clear definition the first
time it appears, then gets used normally. Undefined jargon is a dead end on paper because he
cannot ask. A term defined three times is padding.

**Order the argument so each step is forced.** Before introducing machinery, make the reader feel
the problem it solves. State what you would do naively, show precisely where it breaks, then
introduce the fix. Machinery introduced before its problem reads as arbitrary.

**Concrete instance before general rule.** One worked case, then the pattern. Not the reverse.

**Say what would be different if it were false.** A claim the reader cannot test is a claim they
cannot absorb. Give the observable consequence: "if this were wrong, the histogram would slope
instead of being flat."

**Name the thing that is actually hard.** Every subject has one or two genuinely difficult
points. Find them and spend the words there, rather than distributing effort evenly over things
that are easy.

**Draw the mechanism.** See below. A diagram beside the prose is often the whole difference.

## Voice

Write like a good technical memo or a well-written textbook chapter. Not like chat, and not like
an assistant being helpful.

Cut these on sight:

| Instead of | Write |
|---|---|
| "It's worth noting that the posterior is important here." | "The posterior is the full range of parameter values that could have produced this image." |
| "In this section, we will explore calibration." | "A network can output a confident distribution that is wrong. Calibration is how you find out." |
| "Let's dive into how this works." | (delete; start explaining) |
| "Overall, this approach offers several key advantages." | (delete; state the advantage) |
| "In summary, we have seen that..." | (delete the whole closing paragraph) |
| "I hope this helps clarify things!" | (delete) |

Specifically:

- **No opening sentence that announces what the section is about.** The heading did that. State
  the thing.
- **No closing paragraph that summarises the section it just ended.** He has just read it.
- **No "we will see that", no rhetorical questions to the reader, no "consider the following".**
- **No hedging filler**: "somewhat", "it could be argued", "generally speaking", "in some sense",
  when you actually know the answer. Say it, or say plainly that it is uncertain and why.
- **No inflated register**: delve, leverage, utilise, crucial, robust, comprehensive, seamless,
  landscape, realm, tapestry. Use, use, use, important, reliable, complete, works.
- **No bulleted restatement** of a paragraph that already made the point. Prose carries the
  argument; lists carry things that are genuinely lists.
- **Prefer short declarative sentences** for the load-bearing claims. Save the long ones for
  qualifications.

## Format

Enforced by the tool, so getting it wrong costs you a round trip:

- **No em dashes.** `---` or the character itself is refused with an error. Recast the sentence:
  colon, semicolon, parentheses, or two sentences. `--` for a numeric range is fine.
- **Two columns at 9pt.** A column is about 2.9 inches. Display maths wider than that overflows,
  so break it with `split` or `aligned`. Use `figure*` and `table*` for anything needing the full
  page width. Inside a float, `\linewidth` is the column.
- Available without declaring anything: `amsmath`, `graphicx`, `booktabs`, `enumitem`,
  `hyperref`, `microtype`, `tikz`, `pgfplots`.

## Figures

**Draw them.** He confirmed that hand-written TikZ and pgfplots is what he wants, so do not go
hunting for an existing figure first and do not treat a schematic as second best. Draw the block
diagram of the pipeline, the geometry behind the derivation, the plot of the function under
discussion, the annotated axis showing what a parameter does.

- The screen is **greyscale**. Never encode meaning in colour. The defaults already separate
  plot series by dash pattern and bars by fill level; leave them alone.
- Keep line weights at the preamble default or heavier. Hairlines vanish on e-ink.
- **Say when a figure is illustrative.** A schematic with invented numbers is fine and often the
  clearest thing to draw. A plot that *looks* like a result and is not must say so in its
  caption. That distinction is what makes a drawn figure safe to read on paper.
- A real figure from something he has read is still a good option when it genuinely fits:
  `\includegraphics[width=\linewidth]{<name>}` resolves against the corpus raw store, so an
  ingested figure works by the filename in `figures.raw_path`.

## Before sending

1. Every technical term defined at first use.
2. No em dashes. No assistant register. No section that opens or closes by describing itself.
3. At least one figure if the subject has any structure or geometry to it.
4. Any invented numbers labelled as illustrative in the caption.
5. Equations narrow enough for a 2.9in column.
6. Report the device path the tool returns.
