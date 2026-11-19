# Home swipe: moving between the groups with a swipe

A sideways swipe moves Home to the next or previous group (the tabs: Ask & Create, Build, Media, ...), with a short slide.

## What reads the swipe

* Touchpad: raw multi-touch from evdev (`ABS_MT_*`, per slot), read by the one shared input module (`iyakki-apps/src/touchpad.rs`; the kernel gives no libinput-style gesture events here). With exactly the chosen number of fingers down (three by default, two when chosen), the fingers' mean movement is summed. A swipe is sent once per touch when the sum is at least 18 % of the pad's width sideways and at least twice as far sideways as up or down. A touch that turns vertical (3 % of the width and more vertical than sideways) is a scroll for good, a different number of fingers is another gesture, and the swipe is never sent twice for one touch. Three fingers do not swipe while three-finger drag is on (the drag wins; pick two fingers).
* Touch screen: one finger, in the same touch code that already scrolls (`iyakki-apps/src/touchscreen.rs`): a quick drag at least 140 pixels sideways and twice as far as up or down sends the swipe with the point where the finger landed.

Both become `Event::Swipe { back, at }` (`back` = the fingers moved right; `at` = the start point for a finger, `None` for a pad).

## What takes it

Only the shell, only on Home (`Shell::swipe_event`, `shell_home.rs`), and no app ever sees the event, so apps keep their own gestures. It is refused when: another screen or an app is open (so lists, the model picker and every app dialog are untouched), a menu, dialog, card or pane layout is up, a text field has focus, the volume slider or the dock has the keyboard, the screen is locked or nobody is signed in, or the setting is off. A finger swipe counts only when it began on the group area (the island). The slide is about 0.2 s and is off with reduced motion.

## The setting

Settings, Keyboard: `A` turns the swipe on or off, `C` picks two or three fingers. The assistant: `keyboard_set` with `home_swipe` (boolean) and `home_swipe_fingers` (2 or 3); it asks the person first like every keyboard change, `keyboard_status` tells the state. Saved as `keyboard.home_swipe` and `keyboard.home_swipe_fingers` (on and three by default; old files load unchanged).

## How an app can opt in later

Not built yet (no app needs it). The plan: a default method on `App` such as `fn swipe(&mut self, back: bool, cx: &AppCx) -> bool { false }`. The shell would offer the swipe to the open app first (not while a dialog is up) and only when the app declares it in its manifest or tile table; an app that returns `true` takes the swipe, anything else is ignored (Home's own swipe never runs over an app). The app must say what it does in its own screen, and a vertical-dominant movement must still reach it as a scroll. Until then an app that wants sideways paging uses the touch screen's two-finger swipe (Ctrl+Left/Right) or its own keys.

## Not proven

Tests use made-up event streams. The distance, the dominance rule and the slide's feel need a real trackpad (the Lenovo and the MacBook Pro have one) and a touch screen.
