//! Types moved out of `iyakki_agent::jobtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// One paragraph for the assistant's system prompt (and, worded the same, for an external agent's instructions).
pub const PROMPT_RULE: &str = "\nJOBS: anything that may take longer than about 30 seconds (an SDK or emulator install, a big build, a download, a long test run, a server you start) goes through job_start, never through a foreground command: it runs detached, keeps running when this chat or the session ends, and the person sees it in the Jobs strip with its progress and Log and Stop buttons. After job_start do not poll with sleep or until-loops: call job_wait (up to 60 s, it returns early when the job ends) or job_status, and read the output with job_tail. You are told when a job finishes. Never start the same job twice: check job_list first. job_stop ends one (the person approves).";
