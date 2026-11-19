//! Types moved out of `iyakki_agent::linuxsandboxtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nLINUX SANDBOXES: linux_sandbox_create makes a Debian or Ubuntu sandbox (a background job that downloads the base; progress is beside the clock) and linux_sandbox_delete removes one with everything in it. Both ask first, every time. Give a name (lower-case letters, digits and -) and a release from linux_sandboxes' list (debian-12, debian-13, ubuntu-22.04, ubuntu-24.04); turn the network on only when the person asks. To delete, repeat the sandbox name in confirm_name.";
