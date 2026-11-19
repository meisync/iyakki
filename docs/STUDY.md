# Study plans and quizzes (the LEARN tile and the chat)

You can ask the chat for a lesson plan or a quiz by talking. The assistant writes the content; the program does everything that has to be exact: the dates, the counting, the checking and the score. The LEARN tile and the chat read and write the same store, so what you make in one is in the other.

## What you can say

- "I want to learn programming basics in 21 days." The assistant writes 21 sessions, one idea a day with a small practice task, a review day and a final project, and saves the plan.
- "A yoga session every day for a month", "2 hours of guitar a week", "a 3 month course on first aid". Units are days, hours, weeks or months.
  - days: one session per day (or per weekday, or per week with `every`).
  - weeks: one session per week.
  - hours: the hours are cut into sessions of the length you want (30 minutes unless you say otherwise; the last one is what is left), for example 5 hours in sessions of 45 minutes is 7 sessions.
  - months: one session per week over the real calendar months (1 February to 1 March is 4 sessions; 30 October to 30 November is 5).
- Starts tomorrow at 18:00 unless you give a date and time. At most 366 sessions.
- After a plan is made the assistant **asks** whether to add it to your calendar. It never does so silently. If you say yes it adds one event per session (tagged as the assistant's, with the plan in the notes); "remove it from the calendar" takes out exactly those events and nothing else. Deleting a plan removes its events too.
- "Quiz me with 10 questions on the solar system." The assistant writes multiple-choice questions with an answer and an explanation each. You can take the quiz **in the chat** (one question at a time; you answer with a letter and the program says right or wrong and why) or **in the LEARN tile**. "Open the quiz" leaves an inbox note that opens it in the tile.

## The checks (what the assistant gets told when it is wrong)

A plan is refused, with the exact number it needs, when the number of sessions is not right, when a title is empty or repeated, when a date or time cannot be read, or when it would have more than 366 sessions. A quiz is refused, with a list of every problem, when the number of questions is not the number asked for (1 to 50), a question has no text, there are fewer than 2 or more than 6 options, two options are the same (capitals ignored), an option is "all of the above" or "none of the above", the answer is missing, out of range or more than one, an explanation is missing, a question is repeated, or a text is too long. The assistant is told to check each answer number against its option before it sends the quiz; the program cannot know whether the key is *true*, only that it is well formed.

## Wrong answer keys

A model can still get a fact wrong. In a quiz you can press **Report a wrong question** (key `r`) after you answer, or on the result screen. That question stops counting in every score, past attempts included, and the percent is worked out over the rest. Reporting again takes the report back. In the chat, say so and the assistant calls `quiz_report`.

## The lesson of a day

Click a day of a plan (or choose it and press Enter, or ask the chat to open it) and the tile opens the **lesson of that day**, like TALES writes a story. There is no extra click: the page opens straight into **What you will learn today** (made by the program from the day's title and notes, with the day before and after, so it is there at once) and the assistant's teaching streams in under it, section by section: **Explanation**, **Worked example**, **What to do** (numbered steps), **Practice task** and **Recap**, with one to three **pictures** that each have a caption.

- The lesson is written the first time you open the day and then **saved with the plan** (`<data dir>/study/<user>.lessons/<plan>-<day>/lesson.json`, owner-only, written whole or not at all, the pictures as files beside it). Opening it again is instant and needs no model, also offline.
- **Pictures** come the way TALES gets them: the picture model if one is set up; otherwise the writing model draws a small SVG that goes through the same sanitiser as TALES' drawings; otherwise the program draws a plain placeholder, so a lesson never lacks its pictures. The assistant that writes is the chat model (Settings, Models).
- The program, not the model, strips markup (tags, links, markdown), and caps the sizes: at most 8 sections of 1800 characters, 9000 in all, 3 pictures, captions of 120 characters.
- **Mark day done** ticks the day in the plan (the chat's `study_plan_list` shows it). **Previous / Next section** (Left / Right) move between the sections. **Ask a question** sends a follow-up about this lesson and writes the answer into the page; the questions and answers are kept with the lesson (the last 12). **Make it simpler** and **Go deeper** write the lesson again at that level; **Write it again** asks first. A new lesson replaces the old one only when it is complete: if it fails, the old one stays. **Read aloud** uses the same reader, speech engines and speaker as TALES, and says plainly when no voice is set up. **Cancel** stops the writer; a failure shows its plain reason (no model configured, the model's error) with **Retry**.
- Keys: Left / Right sections, Space marks done, Enter next section (or Retry), `q` ask, `s` simpler, `d` deeper, `g` write again (then `y` / `n`), `r` read aloud, `c` cancel, Up / Down / PgUp / PgDn scroll, Esc steps back (the question box, the confirmation, then the plan).

## The LEARN tile

| Where | Keys and clicks |
|---|---|
| Lists | Left / Right change between PLANS and QUIZZES, Up / Down choose, Enter opens a plan or starts a quiz, `x` twice deletes |
| A plan | Up / Down choose a session, Enter or a click opens its lesson, Space (or the Tick button) ticks it done, `c` adds the plan to the calendar or takes it out, `x` twice deletes, Esc back |
| A question | `a`-`f` or `1`-`6` pick an option, or Up / Down and Enter, or click; then Enter for Next. The right answer and the explanation appear only after you answered. Esc stops the quiz (nothing is saved) |
| The result | Score, percent and each question right, wrong, skipped or reported; Up / Down choose, `r` reports, `a` tries again, Enter leaves |

The tile looks at the store again every couple of seconds, so a plan or quiz the chat made shows up without reopening it.

## Chat tools

`study_plan_create`, `study_plan_list`, `study_plan_progress`, `study_plan_to_calendar` (asks first), `study_plan_calendar_remove` (asks first), `study_plan_delete` (asks first); `quiz_create`, `quiz_list`, `quiz_results`, `quiz_next`, `quiz_answer`, `quiz_report`, `quiz_open`, `quiz_delete` (asks first); `study_lesson_open` (`plan`, `session`) leaves an inbox note that opens the lesson of a day (the tile writes the lesson itself; the chat does not). They belong to the calendar tool group, which is offered when a message mentions a plan, lesson, study, quiz, course and so on, or when the assistant asks for it with `more_tools`. The two calendar tools exist only for accounts that may use the calendar.

## Where it is kept

`<data dir>/study/<user>.json`, one file per person, written whole or not at all with owner-only permissions. The written lessons are in `<user>.lessons/` beside it. It holds plans (sessions with date, time, length, title, notes, done), quizzes (questions, options, key, explanation, reports) and the attempts. It is part of nothing else; a person's backup does not include it yet.

## Not covered

No real model has written a lesson or drawn its pictures yet (only scripted ones), so the quality of the text and of the pictures is unknown; a picture model's own pictures were not tried at all, and the voice was tested with a fake speaker. The lesson page has not been looked at on a real screen. A lesson is not included in a backup yet. Everything here is tested with made-up data and fake time zones; no real language model has written a plan or a quiz through these tools yet, and the tile has not been looked at on a real screen or a device. The plan cards and quiz cards in the chat are plain text, not pictures; "open the tile" is an inbox note the person taps.
