//! Types moved out of `iyakki_agent::linuxappimagetools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const TOOL: &str = "linuxapp_image";

pub const PROMPT_RULE: &str = "\nLINUX APP IMAGE: linuxapp_image turns the archived root of a sandbox (a .tar.gz the person made inside it, inside their own home) into an image app in the dev registry, so it shows in Linux apps -> Image apps. It asks first and is for administrators. Ask the person for: the archive path, an app name, the port the program listens on (1024 or higher), its licence, and the command that starts it (a full path such as /usr/bin/myapp). Never invent any of these.";
