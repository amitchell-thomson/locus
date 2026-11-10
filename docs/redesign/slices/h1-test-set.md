# H1 test set: what a good answer contains (draft for your corrections)

**State:** drafted 10 Nov 2026 by Claude, waiting for your corrections (H1 D8) · **Questions:** heuris.tex J

Each question is sorted into one of three groups:
- **H1:** the index and tools can answer it now;
- **H2 / L:** it needs facts or the learning system, so it is scored there;
- **no data:** the answer must say the data does not exist.

Each H1 question has one line on what a good answer contains. Strike or rewrite any line; a
`?` marks where I do not know the truth and you should fill it in. Each H1 question is then asked
in a fresh `claude -p` with only the locus tools. Each answer is scored on:
- the right current state;
- the right sources found;
- nothing invented.

You spot-check ten.

## Yours

| # | Group | A good answer contains |
| --- | --- | --- |
| Y1 | H1 | Both offers from mail: BH's 20 Aug offer for 21 Jun–27 Aug 2027 (pay, allowance, bonus) and Millennium QD (offer accepted, right to work confirmed 5 Nov). Your own reasoning from past chats about each, and the case for each in terms of your stated goals (systematic, then a graduate role). It notes that BH's contract is signed (applications file, 26 Aug), so choosing Millennium means reneging. |
| Y2 | H1 | The role preference form as you filled it in, quoted from mail or your notes. If it is not found, the answer says so and does not reconstruct it. The QD-versus-QR reasoning grounded in that form and the spring-week mail. `?` Where did the form go: mail, or a form site? |
| Y3 | H1 | Your BH internship notes (tablet notebooks, in the library) cited by page. Ideas tied to nabla's real design from its repo docs, plus other projects only where a real link exists. |
| Y4 | H1 | The B14 image-processing problem sheet and notes, cited. Convolution and CNNs explained from them, with a quant-finance example labelled as Claude's own, not yours. |
| Y5 | H1 | The deep-learning attempts or decisions found in repo docs and chats (nabla, quast, regime-ml, tanker-flow), each cited. Why each stopped, then concrete places where it could help. |
| Y6 | H1 | Your projects and ideas from repos, chats and notes, with an honest take on which could become a startup. Grounded in what each project actually does. |
| Y7 | H1 (laptop) | What needs a reply now: mail tagged needs-reply, plus threads where the last message is to you. BH's state: offer accepted 26 Aug, documents sent 3 Oct, contract pending the background check. Reneging is explained as general knowledge, with that labelled. |
| Y8 | L | |
| Y9 | H1 + L | The Jump feedback notes, found and cited. Weaknesses mapped from them, and next year placed after the Millennium internship. The curriculum itself is L. |

## Facts and current state

| # | Group | A good answer contains |
| --- | --- | --- |
| 1 | H1 | Careers events and tasks ahead from kairos, and dated asks in recent careers mail. On 10 Nov: the GR team mixer 11 Nov 17:00 (not in the calendar, and it clashes with training), team formation by 13 Nov, the passport renewal you told Millennium about. |
| 2 | H1 | Sarah Scadding, 5 Oct, "No problem at all…", in reply to your 3 Oct email with the documents. |
| 3 | H1 | Mail where you agreed to a time or a task, checked against kairos; each one missing from the calendar is listed with its pointer. |
| 4 | H1 | The latest quast commits and chat session, with dates, and what you were in the middle of (your last prompt there). |
| 5 | H1 | nabla's docs or commits or chats that record the decision, quoted with its date. If none record a reason, the answer says so. |
| 6 | H1 | The CV attachment on the G-Research application mail (the file name and date), matched to the cv repo's history if possible. `?` Did it go by mail or through a portal? |

## Linking (7–13), describing your work (14–17), exact source (18–21)

| # | Group | A good answer contains |
| --- | --- | --- |
| 7 | H1 | Named B-course lectures this term (library, calendar), each tied to a specific part of quast's method, both cited. |
| 8 | H1 | B15 Control topics from the notes, matched to nabla's regime work from its docs. Or "nothing useful", with the reason. |
| 9 | H1 | Your notes or papers on nowcasting or shipping, tied to tanker-flow's docs. Or it says there are none. |
| 10 | H1 | Margin notes from the library, matched to live projects (quast, locus, OQTS, GR AI/ML, 3YP), each cited. |
| 11 | H1 | Asked with a real error pasted: a past chat or commit where the same error was fixed, by its pointer. Or it says there is none. |
| 12 | H1 | BH notes ideas applied to quast and OQTS specifically, cited to the notes and the repo docs. |
| 13 | H1 | Papers you read and noted, each tied to a project or a new idea, cited. |
| 14 | H1 | Three bullets whose numbers all appear in nabla's docs or commits, each traceable. |
| 15 | H1 | quast's current best result and its starting point, both from repo docs and commits with dates. |
| 16 | H1 | This week's locus commits, summarised: the files sync, the Home chat, Home live, Slice 0, H1. |
| 17 | H1 | oxdaq from the oqts repos' docs: what it does, its architecture, its state now. Nothing beyond the docs. |
| 18 | H1 | The statement quoted from the library section, cited `src:<doc>#s<n>`. |
| 19 | H1 (partial) | The B14 slides section that explains convolution, cited. `open` returns text; the image comes from `markups` only if the slides are on the tablet. A gap for H1. |
| 20 | H1 | Your margin notes on *Advanced Portfolio Management* about the topic, quoted with page. |
| 21 | H1 (partial) | The section of your B14 notes on 2D Fourier transforms. Opening the page itself goes through `markups`. |

## Mail and acting (22–26)

| # | Group | A good answer contains |
| --- | --- | --- |
| 22 | H1 (laptop) | The threads needing a reply, with a draft for each made through keryx (`draft_reply`), never sent. |
| 23 | H1 | The 3YP group's last proposal, found by search, scheduled through kairos (pending your approval). |
| 24 | H1 | Start date and terms quoted from the Millennium offer mail. `?` Is the offer in mail, or only in a portal? |
| 25 | H1 | Threads where your message was last, asked a question and has had no answer for over a week. |
| 26 | H1 | `timeline(person)` for the person, summarised, with open items. |

## Learning (27–32): L. 31 (years 1 and 2) needs the syllabi facts: H2.

## Careers (33–35)

| # | Group | A good answer contains |
| --- | --- | --- |
| 33 | H1 | Question types drawn from your interview notes and prep chats, counted, each cited. |
| 34 | H1 | A paragraph using real project facts from the repos, cited, for the named firm. |
| 35 | H1 + H2 | The acceptance mail and its challenges list, your projects, and the Millennium offer. Your stated preferences need `remember` (H2): until then it uses your chats about them, cited. |

## Planning and advice (P1–P5)

| # | Group | A good answer contains |
| --- | --- | --- |
| P1 | H1 | The live projects, what is due, and the week's free time, giving a split with reasons. |
| P2 | H1 | A week plan around lectures, live projects and due work, scheduled through kairos (pending). |
| P3–P5 | L | |

## Patterns over time (36–40), personal (41)

| # | Group | A good answer contains |
| --- | --- | --- |
| 36 | H1 | kairos's measured project time for last month against planned blocks, per project. |
| 37 | H1 | Open tasks and mail older than two weeks with no movement, listed with dates. |
| 38 | H1 | September from the timeline: the calendar, commits, chats and mail, summarised by area. |
| 39 | H1 | Project ideas from chats and notes over the year, each dated and cited. |
| 40 | H1.10 (phone) | Tomorrow's calendar and anything to prepare, from claude.ai on the phone. |
| 41 | H1 | Family calendar events over Christmas against yours, with any clash named. |

## Integrity (42–48)

| # | Group | A good answer contains |
| --- | --- | --- |
| 42 | H2 | |
| 43 | H2 | |
| 44 | H1 | The BH offer email is found (it was: Sarah Scadding, 20 Aug). |
| 45 | no data | It says the notes do not exist. |
| 46 | H1 | It names both Sams, or asks which (the test passes on fixtures; checked on real data at scoring). |
| 47 | H1 | Nothing from OQTS-shared. **Open:** the index never holds it, but keryx's own tools do return it (the Home chat read an OQTS-shared thread on 10 Nov). |
| 48 | H1 | Every claim matches quast's repo docs. |
