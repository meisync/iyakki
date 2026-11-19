//! Types moved out of `iyakki_agent::componentstools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSYSTEM COMPONENTS: components_status shows what the person's pinned private stores offer for the kernel, boot, base system, core and kernel layer, what is installed, and the plan (sizes, only-the-difference or whole file, whether a restart is needed, and anything the installed parts cannot support). It only reads the stores' signed lists; it never downloads or installs a component. components_apply installs that plan after the person approves the card that shows it (what changes, sizes, restart needed, what is kept to go back); it always asks and the computer never restarts by itself. Do not promise a component will be installed before the person approved the card.";
