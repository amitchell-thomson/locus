"""Draw the ink-confirm fixture's crop: printed dossier text with a hand-drawn mark over it.

    uv run --with pillow python make_ink_crop.py

A synthetic stand-in for a real crop (D4: nothing of yours in a committed file). It has what a
real one has that makes placing the mark hard: a note in the margin, a curly bracket over part
of a paragraph with its point toward the note, and an arrow that ends between two passages.
Greyscale, like the tablet.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

OUT = Path(__file__).resolve().parent.parent / "default" / "ink" / "quast-dossier-p5.png"
W, H = 1000, 330
SERIF = "/System/Library/Fonts/Times.ttc"
HAND = "/System/Library/Fonts/Supplemental/Bradley Hand Bold.ttf"

TEXT = [
    ("D8", "The fit check restarts the crowd from the prior when no"),
    ("", "candidate explains the data seen so far (§8as)."),
    ("D9", "Delayed acceptance stays off by default until the learned-"),
    ("", "law crowd is shown unbiased; it takes a third off (§8bw)."),
    ("E11", "The shortfall sweep: fidelity 0.93 against a 0.99 target,"),
    ("", "not yet characterised."),
]


def main() -> None:
    img = Image.new("L", (W, H), 250)
    d = ImageDraw.Draw(img)
    serif = ImageFont.truetype(SERIF, 26)
    small = ImageFont.truetype(SERIF, 20)
    hand = ImageFont.truetype(HAND, 34)
    x0, y = 70, 40
    for tag, line in TEXT:
        if tag:
            y += 18
            d.text((x0 - 52, y + 4), tag, font=small, fill=120)
        d.text((x0, y), line, font=serif, fill=30)
        y += 36
    # The page edge: the note sits beyond it, as margin notes do.
    d.line([(735, 0), (735, H)], fill=200, width=2)
    # A curly bracket over the second half of D9, its point toward the note.
    bx, top, bot = 712, 168, 236
    mid = (top + bot) // 2
    pen = 55
    d.arc([bx - 14, top, bx + 14, top + 28], 270, 0, fill=pen, width=4)
    d.line([(bx + 14, top + 14), (bx + 14, mid - 14)], fill=pen, width=4)
    d.arc([bx + 14, mid - 28, bx + 42, mid], 90, 180, fill=pen, width=4)
    d.arc([bx + 14, mid, bx + 42, mid + 28], 180, 270, fill=pen, width=4)
    d.line([(bx + 14, mid + 14), (bx + 14, bot - 14)], fill=pen, width=4)
    d.arc([bx - 14, bot - 28, bx + 14, bot], 0, 90, fill=pen, width=4)
    # The note, and an arrow from it that ends between D9 and E11.
    d.text((770, 150), "is this", font=hand, fill=pen)
    d.text((770, 190), "still true?", font=hand, fill=pen)
    d.line([(800, 240), (760, 300), (720, 268)], fill=pen, width=4, joint="curve")
    d.line([(720, 268), (736, 262)], fill=pen, width=4)
    d.line([(720, 268), (728, 284)], fill=pen, width=4)
    img.save(OUT, optimize=True)
    print(OUT, img.size)


if __name__ == "__main__":
    main()
