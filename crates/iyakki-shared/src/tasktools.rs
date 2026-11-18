//! Types moved out of `iyakki_agent::tasktools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// One line for the lead assistant's system prompt.
pub const PROMPT_RULE: &str = "\nPARALLEL TASKS: for independent pieces of work (\"do these three things in parallel\") call task_spawn: each task runs in its own isolated copy of the workspace and the person reviews and merges the result in the Tasks tab. Tasks cannot see this chat, so write each goal completely. task_bring_changes copies the persons uncommitted files into a stopped task. task_status shows how they are doing and, when all have finished, what each did, the conflicts and what to merge. For a plan with several steps call task_plan with each step's title, goal, the files it changes and which steps it depends on: independent steps with separate files run in parallel, the rest follow after a merge. You cannot merge or discard them.";
