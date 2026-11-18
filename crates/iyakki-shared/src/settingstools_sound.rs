//! Types moved out of `iyakki_agent::settingstools_sound` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSOUND OUTPUT (Settings -> Sound): sound_output_status (free) says which output the sound plays on and why it was chosen, whether the output chosen by hand is set aside (an HDMI/DP output whose pin has no monitor is not played on while another output can play, and it comes back when a monitor is there), and lists every output with its key and whether a monitor or device is connected. sound_output_set takes `output`: `automatic` (follow what is connected) or a key from the list such as `PCH:3`; it asks first (administrators only). A monitor that was just plugged in becomes the output only by the automatic order: speakers or headphones, a monitor with speakers, USB, Bluetooth, the built-in speaker. Volume and mute are not part of these tools.";
