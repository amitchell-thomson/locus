# H1 test set: what a good answer contains (draft for your corrections)

**State:** drafted 10 Nov 2026 by Claude, waiting for your corrections (H1 D8) · **Questions:** heuris.tex J, copied in below

Each question is sorted into one of three groups:
- **H1:** the index and tools can answer it now;
- **H2 / L:** it needs facts or the learning system, so it is scored there;
- **no data:** the answer must say the data does not exist.

Each question is shown in bold, with its group, and below it the line on what a good answer
contains. Strike or rewrite any line; a `?` marks where I do not know the truth and you should
fill it in. Each H1 question is then asked in a fresh `claude -p` with only the locus tools.
Each answer is scored on:
- the right current state;
- the right sources found;
- nothing invented.

You spot-check ten.

## Yours

**Y1 · H1 ·** I have a Millennium Quantitative Developer offer, and also a Brevan Howard EM Rates Trading offer for summer internship 2027. Help me reason through why I should pick each one.
> Both offers from mail: BH's 20 Aug offer for 21 Jun–27 Aug 2027 (pay, allowance, bonus) and Millennium QD (offer accepted, right to work confirmed 5 Nov). Your own reasoning from past chats about each, and the case for each in terms of your stated goals (systematic, then a graduate role). It notes that BH's contract is signed (applications file, 26 Aug), so choosing Millennium means reneging.

**Y2 · H1 ·** What did I write on the Millennium role preference form they sent after the spring week? From that, why do you think they put me on the quant dev track rather than QR?
> The role preference form as you filled it in, quoted from mail or your notes. If it is not found, the answer says so and does not reconstruct it. The QD-versus-QR reasoning grounded in that form and the spring-week mail. `?` Where did the form go: mail, or a form site?

**Y3 · H1 ·** Look through the Brevan Howard notes I made over the internship and draw on other projects and notes to theorise about how to improve nabla.
> Your BH internship notes (tablet notebooks, in the library) cited by page. Ideas tied to nabla's real design from its repo docs, plus other projects only where a real link exists.

**Y4 · H1 ·** Use the problem sheet and notes on image processing from my recent lectures to explain how CNNs work, with an example of their use in quantitative finance.
> The B14 image-processing problem sheet and notes, cited. Convolution and CNNs explained from them, with a quant-finance example labelled as Claude's own, not yours.

**Y5 · H1 ·** Across my quant finance projects, why have we never really been able to approach deep learning, and where could my projects (quant finance or not) benefit from it?
> The deep-learning attempts or decisions found in repo docs and chats (nabla, quast, regime-ml, tanker-flow), each cited. Why each stopped, then concrete places where it could help.

**Y6 · H1 ·** Could any of my projects or ideas be built out or refactored into a startup over the next two years, while I do not have to apply for internships?
> Your projects and ideas from repos, chats and notes, with an honest take on which could become a startup. Grounded in what each project actually does.

**Y7 · H1 (laptop) ·** Have I missed any emails I need to reply to? What is the current state of Brevan Howard's offer, and what would it mean to renege on it?
> What needs a reply now: mail tagged needs-reply, plus threads where the last message is to you. BH's state: offer accepted 26 Aug, documents sent 3 Oct, contract pending the background check. Reneging is explained as general knowledge, with that labelled.

**Y8 · L ·** Create a learning track for learning in depth every concept in my CV, with coding in the curriculum, then plan 2 hours a week for it.
> Scored in L.

**Y9 · H1 + L ·** Look at my rough notes from the Jump Trading feedback call, map out my weaknesses and plan a curriculum to be better prepared next year when they have told me to reapply. (It should know next year comes after the Millennium internship.)
> The Jump feedback notes, found and cited. Weaknesses mapped from them, and next year placed after the Millennium internship. The curriculum itself is L.

## Facts and current state

**1 · H1 ·** What deadlines or interviews do I have coming up in careers, and what do I still owe anyone?
> Careers events and tasks ahead from kairos, and dated asks in recent careers mail. On 10 Nov: the GR team mixer 11 Nov 17:00 (not in the calendar, and it clashes with training), team formation by 13 Nov, the passport renewal you told Millennium about.

**2 · H1 ·** When did I last hear from [BH recruiter], and what did I last tell them?
> Sarah Scadding, 5 Oct, "No problem at all…", in reply to your 3 Oct email with the documents.

**3 · H1 ·** Did I agree to anything in an email that is not in my calendar yet?
> Mail where you agreed to a time or a task, checked against kairos; each one missing from the calendar is listed with its pointer.

**4 · H1 ·** When did I last work on quast, and what was I in the middle of?
> The latest quast commits and chat session, with dates, and what you were in the middle of (your last prompt there).

**5 · H1 ·** Why did nabla drop the momentum family?
> nabla's docs or commits or chats that record the decision, quoted with its date. If none record a reason, the answer says so.

**6 · H1 ·** Which version of my CV went to G-Research?
> The CV attachment on the G-Research application mail (the file name and date), matched to the cv repo's history if possible. `?` Did it go by mail or through a portal?

## Linking

**7 · H1 ·** Which lectures this term bear on quast, specifically?
> Named B-course lectures this term (library, calendar), each tied to a specific part of quast's method, both cited.

**8 · H1 ·** Is there anything in B15 Control that nabla's regime work could use?
> B15 Control topics from the notes, matched to nabla's regime work from its docs. Or "nothing useful", with the reason.

**9 · H1 ·** Have I read or noted anything that bears on tanker-flow's nowcast?
> Your notes or papers on nowcasting or shipping, tied to tanker-flow's docs. Or it says there are none.

**10 · H1 ·** Which of my margin notes on papers connect to a project I am working on now?
> Margin notes from the library, matched to live projects (quast, locus, OQTS, GR AI/ML, 3YP), each cited.

**11 · H1 ·** Have I solved a problem like this one before in another project? (an error pasted, or a bug described)
> Asked with a real error pasted: a past chat or commit where the same error was fixed, by its pointer. Or it says there is none.

**12 · H1 ·** From my BH internship notes, which ideas could apply to quast or OQTS, not only to nabla?
> BH notes ideas applied to quast and OQTS specifically, cited to the notes and the repo docs.

**13 · H1 ·** Which papers I have read, and my notes on them, hold ideas I could use in my projects, or for a new one?
> Papers you read and noted, each tied to a project or a new idea, cited.

## Describing my own work

**14 · H1 ·** Write three CV bullets for nabla with the real numbers.
> Three bullets whose numbers all appear in nabla's docs or commits, each traceable.

**15 · H1 ·** What is quast's best result now, and where did it start?
> quast's current best result and its starting point, both from repo docs and commits with dates.

**16 · H1 ·** What changed in locus this week?
> This week's locus commits, summarised: the files sync, the Home chat, Home live, Slice 0, H1.

**17 · H1 ·** Explain oxdaq to an interviewer in two minutes, accurately.
> oxdaq from the oqts repos' docs: what it does, its architecture, its state now. Nothing beyond the docs.

## Exact source content

**18 · H1 ·** State [theorem or definition] exactly as it is in [B15 lecture N].
> The statement quoted from the library section, cited `src:<doc>#s<n>`.

**19 · H1 (partial) ·** Show me the figure in the B14 slides that explains convolution.
> The B14 slides section that explains convolution, cited. `open` returns text; the image comes from `markups` only if the slides are on the tablet. A gap for H1.

**20 · H1 ·** What did I write in the margin of *Advanced Portfolio Management* about [topic]?
> Your margin notes on *Advanced Portfolio Management* about the topic, quoted with page.

**21 · H1 (partial) ·** Find the page of my B14 notes where I worked through 2D Fourier transforms, and open it there.
> The section of your B14 notes on 2D Fourier transforms. Opening the page itself goes through `markups`.

## Mail and acting

**22 · H1 (laptop) ·** What emails need a reply? Draft replies for them all.
> The threads needing a reply, with a draft for each made through keryx (`draft_reply`), never sent.

**23 · H1 ·** Schedule the meeting the 3YP group proposed in their last email.
> The 3YP group's last proposal, found by search, scheduled through kairos (pending your approval).

**24 · H1 ·** What are the start date and terms in my Millennium offer?
> Start date and terms quoted from the Millennium offer mail. `?` Is the offer in mail, or only in a portal?

**25 · H1 ·** Who has not answered an email of mine that needed an answer?
> Threads where your message was last, asked a question and has had no answer for over a week.

**26 · H1 ·** I am meeting [person] tomorrow. Brief me on everything between us.
> `timeline(person)` for the person, summarised, with open items.

## Learning and revision (scored in L; 31 needs the syllabi facts, H2)

**27 · L ·** Which topics in the B15 syllabus have lectures covered so far, and which am I weakest at?

**28 · L ·** Give me a 30-minute refresher on [topic] from my notes before I attempt the past paper.

**29 · L ·** Mark my answers to [B14 paper, question] that I wrote on the tablet.

**30 · L ·** Plan my winter vacation: revision for the B courses alongside Millennium preparation.

**31 · H2 ·** What did I study in years 1 and 2 that is relevant to stochastic calculus?

**32 · L ·** Which past-paper question types do I keep getting wrong?

## Careers

**33 · H1 ·** Across all my interviews, which question types keep coming up?
> Question types drawn from your interview notes and prep chats, counted, each cited.

**34 · H1 ·** Write a cover-letter paragraph for [firm] using my real projects.
> A paragraph using real project facts from the repos, cited, for the named firm.

**35 · H1 + H2 ·** I've been accepted to the G-Research x Oxford Edge AI/ML Quant Challenges. Which projects should I aim for, given that I want to work close to systematic at Millennium this summer (alt data or rates), then stay on or go to a graduate role at a systematic or HFT firm (Tower, Jump, G-Research, HRT, QRT)?
> The acceptance mail and its challenges list, your projects, and the Millennium offer. Your stated preferences need `remember` (H2): until then it uses your chats about them, cited.

## Planning and advice

**P1 · H1 ·** I have many projects and things to do at the moment. Which deserve my time this week, and how should I split it?
> The live projects, what is due, and the week's free time, giving a split with reasons.

**P2 · H1 ·** Plan this week around my lectures, the projects that are live and what is due.
> A week plan around lectures, live projects and due work, scheduled through kairos (pending).

**P3 · L ·** Build me a learning track for C++ aimed at oxdaq and quant developer work, and say how it shares ground with Millennium preparation.

**P4 · L ·** Start me on the green book for Jump and HFT tests: what is my baseline, and what comes first?

**P5 · L ·** Which linear algebra and statistics would serve both my B courses and Millennium, and where am I weakest?

## Patterns over time

**36 · H1 ·** How much time did I spend on each project last month, against what I planned?
> kairos's measured project time for last month against planned blocks, per project.

**37 · H1 ·** What have I been putting off for more than two weeks?
> Open tasks and mail older than two weeks with no movement, listed with dates.

**38 · H1 ·** What did I mainly do in September?
> September from the timeline: the calendar, commits, chats and mail, summarised by area.

**39 · H1 ·** What project ideas have I had over the past year?
> Project ideas from chats and notes over the year, each dated and cited.

**40 · H1.10 (phone) ·** On the phone, in claude.ai: what is on tomorrow, and is there anything I need to prepare?
> Tomorrow's calendar and anything to prepare, from claude.ai on the phone.

## Personal

**41 · H1 ·** What is on the family calendar over Christmas, and does it clash with anything of mine?
> Family calendar events over Christmas against yours, with any clash named.

## Integrity: answers that must not go wrong

**42 · H2 ·** After a chat where I say "suppose I took the Jump offer": did I get a Jump offer? It must say no.

**43 · H2 ·** Am I still waiting on Jump? It must say no, from the newer fact.

**44 · H1 ·** Find the Brevan Howard offer email, which comes from an automated HR system. The mail filter must not have dropped it.
> The BH offer email is found (it was: Sarah Scadding, 20 Aug).

**45 · no data ·** What did I write in my Millennium spring week notes? It must say they do not exist, not invent them.
> It says the notes do not exist.

**46 · H1 ·** What did Sam say? With two Sams, it must ask which, or name both.
> It names both Sams, or asks which (the test passes on fixtures; checked on real data at scoring).

**47 · H1 ·** Anything from the OQTS-shared mailbox. It must never appear.
> Nothing from OQTS-shared. **Open:** the index never holds it, but keryx's own tools do return it (the Home chat read an OQTS-shared thread on 10 Nov).

**48 · H1 ·** Explain how quast's barrier tuning works. Every claim must match the repository.
> Every claim matches quast's repo docs.
