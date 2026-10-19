"""Generate the screen mockups as fixed-width text files, checking every line's width."""
import os, sys

OUT = os.path.join(os.path.dirname(__file__), "mock")
os.makedirs(OUT, exist_ok=True)


class Box:
    def __init__(self, width):
        self.W = width          # total width including the two border chars
        self.lines = []

    def top(self, left, right=""):
        inner = self.W - 2
        mid = inner - len(left) - len(right) - 2
        self.lines.append("╭" + left + "─" * max(mid, 1) + right + "─╮" if right else "╭" + left + "─" * (inner - len(left)) + "╮")

    def bottom(self, label=""):
        inner = self.W - 2
        if label:
            pad = inner - len(label) - 1
            self.lines.append("╰" + "─" * pad + label + "─╯")
        else:
            self.lines.append("╰" + "─" * inner + "╯")

    def row(self, *parts, widths=None):
        """One content row. With widths, parts are columns separated by │."""
        inner = self.W - 2
        if widths is None:
            s = parts[0]
            assert len(s) <= inner, (len(s), inner, s)
            self.lines.append("│" + s.ljust(inner) + "│")
        else:
            cells = []
            for p, w in zip(parts, widths):
                assert len(p) <= w, (len(p), w, p)
                cells.append(p.ljust(w))
            s = "│".join(cells)
            assert len(s) == inner, (len(s), inner)
            self.lines.append("│" + s + "│")

    def sep(self, label="", right="", splits=(), up=(), down=()):
        """Horizontal rule. splits: column positions (inner index) with ┼, up: ┴, down: ┬."""
        inner = self.W - 2
        chars = ["─"] * inner
        for i, ch in enumerate(label):
            chars[i] = ch
        for i, ch in enumerate(right):
            chars[inner - len(right) - 1 + i] = ch
        for p in splits:
            chars[p] = "┼"
        for p in up:
            chars[p] = "┴"
        for p in down:
            chars[p] = "┬"
        self.lines.append("├" + "".join(chars) + "┤")

    def save(self, name):
        for l in self.lines:
            assert len(l) == self.W, (name, len(l), l)
        with open(os.path.join(OUT, name + ".txt"), "w") as f:
            f.write("\n".join(self.lines) + "\n")


def free(name, text, width=None):
    lines = text.strip("\n").split("\n")
    if width:
        for l in lines:
            assert len(l) <= width, (name, len(l), l)
    with open(os.path.join(OUT, name + ".txt"), "w") as f:
        f.write("\n".join(lines) + "\n")


# ---------------------------------------------------------------- Home (95 x 36)
W = 95
b = Box(W)
b.top("─ locus ", " sat 18 oct 21:40 ── ◉ 4 running ")
b.row(" ◷ next   Futsal training 18:00 · 2 chores fit today" + " " * 21 + "Alt-3 calendar ▸ ")
b.row(" ✉ mail   2 need a reply · 1 deadline proposed" + " " * 31 + "Alt-4 mail ▸ ")
L = 62   # left column width (inner) for split sections
R = W - 2 - L - 1
b.sep("─ work · this week ", "[w]", down=(L,))
grid = [
    "      06    09    12    15    18    21    24",
    " mon  ·  ·  ░▒▓▓█▓▓ ·  ▓▓▓██▓ ·  ·  ▒▓▓▒ · ·",
    " tue  ·  ·  ·  ▒▓▓▓▓▓█▓·  ·  ·  ▓▓▓▓▓▓▒ · ·",
    " wed  ·  ▒▓▓ ·  ·  ▓▓▓▓▓▓▓▓█▓▓▒ ·  ·  ·  · ·",
    " thu  ·  ·  ▓▓▓▓▓█ ·  ·  ·  ▒▒ ·  ▓▓▓▓█▓ · ·",
    " fri  ·  ·  ▒▓▓▓▓▓▓▓▓▓ ·  ·  ·  ·  ·  ·  · ·",
    " sat  ·  ·  ·  ·  ▓▓▓▓█▓▓ ·  ·  ·  ▓▓▓█▓▓▒ ·",
    " sun",
]
stats = [
    " this week          23.5 h",
    " quast   11.0 h  ━━━━━━━━━━━",
    " nabla    6.0 h  ━━━━━━",
    " locus    4.5 h  ━━━━━",
    " oqts     2.0 h  ━━",
    " blocks kept        8 of 9",
    " streak            11 days",
    "",
]
for g, s in zip(grid, stats):
    b.row(g, s, widths=(L, R))
b.sep("─ projects ", "[p]", up=(L,))
b.row(" ○ nabla       unattended   ‖ needs you · 41m · allow: cargo test --release?      ●1 unread")
b.row(" ● quast       attended     ⟳ working · 12m · running pytest -k barrier           ●2 unread")
b.row(" ○ oqts        unattended   ✓ done · 2h ago")
b.row(" · locus-dev   idle                                                              ●3 unread")
b.sep("─ explore · 4 of 7 ", "", down=(L,))
# fix label of right half
b.lines[-1] = b.lines[-1][: L + 2] + "─ approve 6 " + "─" * (R - 16) + "[a]─┤"
ex = [
    " ▸ ◆ quast   Bayes-opt barrier tuning, 3× fewer measurements",
    "       why: your fidelity gap is barrier control (E11)",
    "       touches D14 · read §4 · 15 min        y ✓  n ✗  r read",
    "   ◇ read    \"The bitter lesson\" · 12 min · adjacent",
    "   ✦ idea    Kalman nowcast on tanker-flow AIS data",
    "   ◆ nabla   regime filter from Control lecture 8",
]
ap = [
    " jot p3 → careers/g-research",
    " 14 mails → archive (spam)",
    " deadline GR apps 31 Oct ← keryx",
    " 3 ink marks: where do they point?",
    " stop asking: spam archive?",
    " + 1 more                  [e] [a]",
]
for e, a in zip(ex, ap):
    b.row(e, a, widths=(L, R))
b.sep("─ system ", "[s]", up=(L,))
b.row(" ⟳ dossier · quast · opus · 12m    ⟳ scout · reading · sonnet · 3m    ✓ 14 today   ✗ 0")
b.row(" budget  ━━━━━━━━━━━━━━━━━━──────────────┊────   9.1% of 15% this week     window 34% · 4%")
b.bottom(" p e a s w  focus · ⏎ open · l log · / search · ? keys ")
b.save("home")

# ---------------------------------------------------------------- Project (95 x 30)
b = Box(W)
b.top("─ quast ─ field ─ ● attended ─ following ◉ ", " ⟳ claude working · 3m ")
b.sep("─ where you are ")
b.row(" Barrier control step 2 runs on the simulator; batteries waived for steps 1–2 (16 Oct).")
b.row(" Fidelity 0.93 against a 0.99 target. The shortfall is not yet characterised.")
b.row(" next ▸ run the shortfall sweep (E11) before trying any new method")
b.sep("─ new · 5 unread ", "[n]")
b.row(" ● ⚠ D14 compass search: worth re-checking · a scout find bears on it                 2h")
b.row(" ● ? tempering · answered from your margin · links ML lecture 9                        5h")
b.row(" ● ◆ Bayes-opt barrier tuning, 3× fewer measurements           y ✓  n ✗  r read      1d")
b.row(" ● ▲ 4 commits on barrier-control · sweep script added                                 1d")
b.row(" ● ✎ dossier rebuilt · §3 ledger and §4 evidence changed · 2 pages to read             1d")
b.row("   ✍ 60 margin marks read back · 14 decisions written to the log                       2d")
b.sep("─ sessions ", "[s]")
b.row(" ⟳ main chat       working · 3m        running pytest -k barrier")
b.row(" ‖ sweep (bg)      needs you · 12m     allow: python sweep.py --full ?         ⏎ jump")
b.sep("─ chores ", "[c]")
b.row(" reply to Cornelius about D1 · 5 min                                 ⏎ open · x done")
b.row(" book GR interview slot · 2 min · due mon                            ⏎ open · x done")
b.sep("")
b.row(" ask › why did we pick SMC over a genetic algorithm?_")
b.bottom(" ⏎ answer here · ⇥ send to tablet · a answered · d decisions · o dossier · m pin · / search ")
b.save("project")

# ---------------------------------------------------------------- System (95)
b = Box(W)
b.top("─ system ", " esc back ")
b.sep("─ running · live ")
b.row(" ⟳ dossier · quast         opus     step 4 of 6 · rendering plots                      12:41")
b.row(" ⟳ scout · reading         sonnet   searching arXiv                                     3:07")
b.row(" ⟳ change record · oqts    haiku    reading diff a1b2c3                                 0:12")
b.sep("─ recent ")
b.row(" ✓ ink readback · brief v1         56 marks · 3 unsure, sent to approve            21:02")
b.row(" ✗ tablet push · nabla dossier     rmapi auth expired · retrying 22:00   ⏎ details  20:40")
b.row(" ✓ change record · quast           4 commits                                       20:31")
T = 46
B = W - 2 - T - 1
b.sep("─ usage ", "", down=(T,))
use = [
    (" this week         9.1%   of 15% cap", " ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━──────────┊──── "),
    (" by sunday        13.2%   projected", " ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┊──── "),
    (" window · you       34%", " ━━━━━━━━━━━━━━━━━──────────────┊───────────── "),
    (" window · locus      4%   stops at 60%", " ━━──────────────────────────────────────────── "),
    ("", ""),
    (" mon    1.8%   dossier, scout", " ━━━━━━━━━━━━ "),
    (" tue    0.9%   answers", " ━━━━━━ "),
    (" wed    2.6%   dossier, ink", " ━━━━━━━━━━━━━━━━━ "),
    (" thu    1.1%   scout", " ━━━━━━━ "),
    (" fri    2.7%   dossier", " ━━━━━━━━━━━━━━━━━━ "),
    ("", ""),
    (" top  dossier · quast · opus     2.1%", " ━━━━━━━━━━━━━━ "),
    ("      scout · reading · sonnet   1.4%", " ━━━━━━━━━ "),
    ("      answers · sonnet           0.9%", " ━━━━━━ "),
    ("", ""),
    (" by project  quast 4.0 · nabla 2.6 · oqts 0.8", " ━━━━━━━━━━━━━━━━━━━━━━━━━━┊━━━━━━━━━━━━━━━━┊━━━━ "),
]
for t, bar in use:
    b.row(t, bar, widths=(T, B))
b.bottom(" t toggle colours by job type · esc back ")
b.save("system")

# ---------------------------------------------------------------- Explore
b = Box(W)
b.top("─ explore ─ queue 4 of 7 · ideas 12 · reading 3 of 5 ", " esc back ")
b.row(" [q]ueue   [i]deas   [r]eading")
Lq = 33
Rq = W - 2 - Lq - 1
b.sep("─ queue ", "", down=(Lq,))
left = [
    " ▸ ◆ quast   Bayes-opt barrier",
    "   ◆ nabla   regime filter",
    "   ◇ read    the bitter lesson",
    "   ✦ idea    Kalman nowcast",
    "", "", "", "", "", "", "", "", "",
]
right = [
    " ◆ for quast · a method for a stuck result",
    "",
    " Bayesian optimisation tunes double-dot barriers with about",
    " 3× fewer measurements than pattern search, on a simulated",
    " 2×2 device.",
    "",
    " why      your fidelity gap is barrier control (dossier §4, E11)",
    " differs  simulated device only; no readout noise modelled",
    " touches  D14 compass search for the barrier step",
    " cost     §4 only · about 15 minutes",
    "",
    " y ✓    n ✗ then reason: i irrelevant · k knew it · l level · t long",
    " r move to reading · p park on the idea board",
]
for l, r in zip(left, right):
    b.row(l, r, widths=(Lq, Rq))
b.bottom()
b.save("explore")

# ---------------------------------------------------------------- Approve + auto log
W2 = 80
b = Box(W2)
b.top("─ approve 6 ", " l log · esc back ")
for line in [
    " filing · 2",
    " ▸ jot p3 \"G-Research sponsor talk\"  → careers/g-research          y  n  ⏎",
    "   jot p3 \"Kalman filters for…\"      → explore                     y  n  ⏎",
    " mail · 2",
    "   14 mails → archive (spam)                                   y  n  ⏎ list",
    "   deadline: GR apps close 31 Oct ← keryx                           y  n  ⏎",
    " ink · 1",
    "   brief v1 · 3 marks I am unsure where they point             ⏎ confirm",
    " stop asking? · 1",
    "   spam archives: you approved 23 of 23. Do them automatically?     y  n",
    "",
    " Y approves a whole group",
]:
    b.row(line)
b.bottom()
b.save("approve")

b = Box(W2)
b.top("─ auto log · live ", " esc back ")
for line in [
    " 21:42  archived 3 mails (spam)                                         u undo",
    " 21:30  change record · quast · 4 commits",
    " 20:58  filed jot p2 → projects/nabla   (automatic since 9 Oct)        u undo",
    " 19:10  kairos moved a work block out of a clash with Futsal           u undo",
    " ── yesterday ──────────────────────────────────────────────────────────────",
    " 23:05  dossier rebuilt · nabla",
    " 18:12  archived 9 mails (spam)                                         u undo",
]:
    b.row(line)
b.bottom()
b.save("autolog")

# ---------------------------------------------------------------- Answered / Decisions
b = Box(W2)
b.top("─ quast · answered ", " esc back ")
for line in [
    " ▸ tempering               ink ?   dossier p4                        5h",
    "   SMC or a genetic alg.   ask     ·                                 1d",
    "   exact posterior         ink ?   dossier p6                        3d",
]:
    b.row(line)
b.sep("")
for line in [
    " Tempering runs the sampler through flattened versions of the posterior,",
    " sharpening step by step, so the particles do not all settle on one peak",
    " too early.",
    " ↳ the same idea as the cooling schedule in simulated annealing",
    "   (ML lecture 9, p.3)",
    "",
    "                                    f follow-up · t send to tablet",
]:
    b.row(line)
b.bottom()
b.save("answered")

b = Box(W)
b.top("─ quast · decisions ─ 2 re-check · 21 holding · 4 re-decided ", " esc back ")
for line in [
    " ▸ ⚠ D14  compass search for the barrier step                       12 Oct   re-check",
    "   ⚠ D9   waive batteries for steps 1 and 2                         16 Oct   re-check",
    "   ✓ D1   device model from Cornelius                                2 Oct   holding",
    "   ↻ D3   genetic algorithm, replaced by SMC                        21 Sep   → D6",
]:
    b.row(line)
b.sep("")
for line in [
    " D14 · compass search for the barrier step · agreed 12 Oct",
    "",
    " chose     compass search: needs no gradient and tolerates readout noise",
    " over      Nelder–Mead (stalls on plateaus) · gradient descent (too noisy)",
    " you       \"keep it simple until barrier control works\"  [chat 12 Oct]",
    " evidence  E9: converges in 40 steps on the simulator  [a1b2c3]",
    " re-check  a scout find: Bayes-opt needs 3× fewer measurements  [18 Oct]",
    " affects   v2 step 2 scope · nothing built on it yet",
    "",
    "                         ? ask why · o reopen · ⏎ full entry in docs/decisions",
]:
    b.row(line)
b.bottom()
b.save("decisions")

# ---------------------------------------------------------------- Changes / Search
b = Box(W2)
b.top("─ quast · changes · since mon ▾ ", " esc back ")
for line in [
    " sat 18  ▲ 4 commits on barrier-control · sweep script added",
    "         ✎ dossier rebuilt · §3 ledger, §4 evidence",
    " fri 17  » chat 2h · decided to waive batteries for steps 1 and 2",
    " thu 16  ✍ 60 margin marks read back · 14 decisions to the log",
    " wed 15  ◆ scout find: Bayes-opt barrier tuning",
]:
    b.row(line)
b.bottom()
b.save("changes")

b = Box(W2)
b.top("─ / barrier tuning_ ", " scope: everything ⇥ ")
for line in [
    " decisions   quast D14 compass search for the barrier step",
    " answers     quast \"why compass and not gradient?\" · 14 Oct",
    " explore     ◆ Bayes-opt barrier tuning · in the queue",
    " notes       projects/quast/jot.tex · 9 Oct · \"barrier gates …\"",
    " library     Optimisation lecture 6 p.12 · a 2024 tuning paper §3",
    " chats       quast · 12 Oct · \"the barrier step keeps stalling\"",
    " dossiers    quast §3 · D14",
]:
    b.row(line)
b.bottom()
b.save("search")

# ---------------------------------------------------------------- Interview prep
b = Box(W)
b.top("─ g-research · quant researcher · final round 4 Nov ", " esc back ")
b.row(" budget  12 h by 3 Nov · 4.5 h used  ━━━━━━━━━━━━━━━━━━──────────────────────────────")
b.row(" format  two technical (probability, ML), one behavioural · from your notes")
b.sep("─ topics ", "")
for line in [
    "                               confidence        from",
    " probability puzzles           ━━━━━━━━━━━━──    Jump Trading log, 14 sessions",
    " ML theory                     ━━━━━━━━──────    B1 ML course",
    " statistics, hypothesis tests  ━━━━──────────    weakest: gets the most time",
    " behavioural stories           ━━━━━━━━━─────    Jump Trading log",
]:
    b.row(line)
b.sep("─ practice log ", "")
for line in [
    " fri 17   statistics · 6 problems · 4 right · slow on power calculations          1 h",
    " wed 15   puzzles · 8 problems · 7 right                                          1 h",
    "",
    " next     sun 19 · statistics, 1.5 h (kairos block)               + log a session",
]:
    b.row(line)
b.bottom()
b.save("prep")

# ---------------------------------------------------------------- Weekly review
b = Box(W)
b.top("─ weekly review · 13 to 19 Oct ", " esc pause ")
b.row(" 1 week ✓ ── 2 decisions ● ── 3 explore ○ ── 4 locus ○ ── 5 next week ○")
b.sep("")
for line in [
    " 2  decisions worth re-checking · 2",
    "",
    " quast · D14 compass search for the barrier step                           agreed 12 Oct",
    "   flagged  a scout find reports Bayesian optimisation needs 3× fewer measurements",
    "            on a similar device (simulated; no readout noise)",
    "   effect   v2 step 2 scope; nothing built on it yet",
    "",
    "   ? ask why it was chosen    o reopen in quast    → next",
    "",
    " nabla · D7 reject the momentum family                                      agreed 3 Oct",
    "   flagged  …",
]:
    b.row(line)
b.bottom()
b.save("review")

free("review_steps", """
1  week ──────────────────────────────────────────────────────────────────────────────
   the week's rhythm grid
   quast 11 h · nabla 6 h · oqts 2 h          blocks kept on their project   8 of 9
   budget 11.4% of 15%                        dossiers rebuilt   quast, nabla

3  explore ── 3 left in the queue · best idea of the fortnight · 2 reading untouched ──
   the same cards as the Explore page, graded in turn; then the best idea as a full
   card:  p park · d discuss · x drop

4  locus itself ──────────────────────────────────────────────────────────────────────
   what did locus get wrong this week?  › _
   you never opened Answered this week. Keep it?                              y  n
   strikes: wrong level ×4 · irrelevant ×1   → the scout is pitching too basic

5  next week ─────────────────────────────────────────────────────────────────────────
   deep blocks   quast ━━━━━━ 3     nabla ━━━━ 2     oqts ━━ 1
                 ← → project · ↑ ↓ blocks · ⏎ plan them in kairos
""", 95)

# ---------------------------------------------------------------- Work section options
free("work_rhythm", """
        06    09    12    15    18    21    24
 mon 13 ·  ·  ░▒▓▓█▓▓ ·  ▓▓▓██▓ ·  ·  ▒▓▓▒ · ·
 tue 14 ·  ·  ·  ▒▓▓▓▓▓█▓·  ·  ·  ▓▓▓▓▓▓▒ · ·
 wed 15 ·  ▒▓▓ ·  ·  ▓▓▓▓▓▓▓▓█▓▓▒ ·  ·  ·  · ·
 thu 16 ·  ·  ▓▓▓▓▓█ ·  ·  ·  ▒▒ ·  ▓▓▓▓█▓ · ·
 fri 17 ·  ·  ▒▓▓▓▓▓▓▓▓▓ ·  ·  ·  ·  ·  ·  · ·
""")
free("work_area", """
⠀⠀⠀⢀⣀⣤⣶⣿⣷⣦⣄⡀⠀⠀⢀⣤⣾⣿⣿⣷⣤⣀⣀⣤⣶⣿⣿⣿⣷⣦⣄⠀⠀⢀⣀⣤⣶⣶⣤⣀   quast
⣀⣤⣶⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣾⣿⣿⣿⣿⣿⣿⣿   nabla
 aug              sep              oct
""")

# ---------------------------------------------------------------- the reading header (on the PDF)
free("reading_head", """
why this, for you ─ adjacent to quast's tuning problem: the same
explore/exploit trade-off, argued from 70 years of AI. You ticked
3 essays like this in October.                     ticked 18 Oct
""")
print("ok", sorted(os.listdir(OUT)))
