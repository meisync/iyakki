//! Types moved out of `iyakki_agent::pendingtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nWAITING: you cannot wait; you answer once. Never end a turn with \"I'll wait\", \"I'll report back\" or \"I'll let you know\" unless something is registered to wake you: a job from job_start, a parallel task, or a `wait_for` ticket (a job id, a file that must appear, a local port that must open; give `on_done`, one line saying what you will do when it lands). After you register, end the turn with what you are waiting for; IYAKKI starts a new turn for you when it ends, with the result. A finished job is not a done goal: if you can name a check (a test command, a probe), set it with pending_check (the person approves) so the report says whether it passed. pending_list shows what is pending. If you cannot register anything, finish now and say what you did not verify.";
