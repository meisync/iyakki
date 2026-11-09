# Accessibility

What works today for a person who cannot see the screen well, or cannot use a mouse, and what does not. Read the last part before you
rely on any of it: **no screen reader user has tried this yet.** Everything below was exercised only through tests with a fake voice and
a fake keyboard (see "How it was checked").

## Turn it on without seeing the screen

Press **Super+Alt+S** (Super is the Windows or Command key). It works on every screen, including the sign-in screen and the lock
screen, and the choice is kept for the computer, so the next start is already speaking. It says "Screen reader on" when it has a
voice. Press it again to turn the reader off.

If you cannot hold three keys at once, turn on **sticky keys** first (Settings, Accessibility, key `k`; or ask the assistant). Then
press Super, let go, press Alt, let go, press S.

The reader speaks with the reading voice of **Settings, Voice**. If no voice is set up it cannot speak, and it says so **on the
screen** (a small sign at the bottom right, and a message), because that is all it can do: a sighted helper has to open Settings, Voice
and choose a voice. This is the main gap for a first start on a new computer.

## The keys

| Key | What it does |
|---|---|
| Super+Alt+S | Screen reader on or off, anywhere |
| Super+Alt+Right (or Down) | Next control on the screen: top to bottom, left to right. A ring is drawn round it; the reader says it |
| Super+Alt+Left (or Up) | Previous control |
| Super+Alt+Enter | Press the marked control (the click a mouse would send to it) |
| Super+Alt+R | Say the last thing again |
| F1 | The keys of the screen you are on (also read aloud) |
| Tab, arrows, letters | Each screen keeps its own keys, as before |

The walk works without the screen reader too, for a person who cannot use a mouse: every control that can be clicked is a stop on it.

## What the reader says

* The control you walked to: "Save, button", "Dark mode, switch, on", "Volume, slider, 40 percent", "Network, tab, selected".
* A text field that took the focus, and its text: "Name, edit, Ann". Typing is **not** read back key by key (no typing echo yet).
* A **password** field says "Password, password field, 5 characters typed". The characters are never given to the speech queue; the
  model that carries them keeps only the count.
* A new screen or page ("Settings, Display"), then the first control with the focus.
* The chosen row of a list when the arrow keys move it, on screens that use the toolkit's list rows.
* Toasts, the sign-in error, notices and the messages Settings pages show (switch these off with "Read messages and errors aloud").
* A new focus **interrupts** what is being said. A notice waits for what is being said and is dropped if you move on first.

## Settings, Accessibility

Open it with F7 then the page list, or search "accessibility". Everyone has it; no administrator is needed.

| Key | Choice |
|---|---|
| `r` | Screen reader (same as Super+Alt+S) |
| `t` | Test the voice (says a sentence, or says why it cannot) |
| `n` | Read messages and errors aloud |
| `c` | **High contrast**: black and white, a yellow accent, thick borders. It is the theme of Settings, Display; every text pair is at least 7 to 1 (WCAG AAA), checked by a test that computes the ratios |
| `u` / `U` | Bigger / smaller text and buttons (the UI size of Settings, Display: 85, 100, 125, 150, 200 percent) |
| `p` | Large mouse pointer (two more pixels per pixel of the pointer picture) |
| `f` | Big focus ring (twice as thick) |
| `m` | Reduce motion: no moving background or particles, a caret that does not blink |
| `k` | Sticky keys: Shift, Ctrl, Alt and Super stay on for the next key. Twice in a row keeps one on until a third press |
| `w` | Slow keys: a key counts after it was held 300, 500, 800 or 1200 ms (cycles, back to off). A brush against a key does nothing |

The delay and the speed of a held key repeating are in **Settings, Keyboard** (administrators). They were not copied here.

**By chat:** `accessibility_status` shows every choice and whether a reading voice exists (it asks nothing); `accessibility_set` changes
any of them (screen reader, notices, high contrast, large pointer, big focus ring, reduce motion, sticky keys, slow keys) and **asks
every time**. The assistant cannot hear the voice, so it says what the status shows and never claims the reader works.

Where the choices are kept: `access-<name>.json` (yours), `access-machine.json` (only "screen reader on", so the sign-in screen
knows), and the theme and size you already had in `ui-<name>.json`. The shell reads them again within a second of a change.

## For developers of tiles

The toolkit (`iyakki-uikit::access`) builds a node for every clickable region of a frame: the **visible label is the accessible
name**, the role comes from the kind of event (a key or tab id is a button, a row id an item, a field id a field), and the state is
empty. Most tiles need nothing. To say more, call `access::describe(label, &event, Meta::new(Role::Toggle, State::default().checked(on)))`
right after registering the hit (the switch, slider and text field of the toolkit already do). A tile that wants to say something that is
not a control calls `access::announce("Export finished")`. A chosen list row drawn with `fx::row_at` is noticed on its own.

## How it was checked

* Tests compute the contrast of every text colour on every ground of the three themes (WCAG ratios, not by eye) and of a real button.
* The speech queue, the node text, sticky keys, slow keys and the walk order are pure code with unit tests; the reader is driven in
  the shell with a **fake voice that records** what it was asked to say, at the sign-in screen, in a walk, on a new screen, with a
  toast, and with a password.
* A test walks the main screens (sign-in, home, lock) and **every Settings page** at two screen sizes and compares the walk with the
  list of clickable regions: nothing clickable is left out. The tests also print how many controls of each page have a key of their
  own and how many only the walk reaches.
* The key chords and sticky and slow keys are tested on raw key events, not on a physical keyboard.

## Honest gaps

* **No real screen reader user has tried it.** The wording, the speed, the order of the walk and the amount of speech are guesses.
* **Nothing has been heard on a real speaker.** The voice is the Settings, Voice reading voice; its quality and delay are those of
  that engine (a local engine that starts slowly makes the reader slow).
* **Web pages in the Browser tile are not exposed.** The reader sees the browser's own address bar and buttons, not the page. A web
  page's headings, links and forms are not spoken. (A page has its own dark and high-contrast rules; the browser tile's colours do not
  follow this theme.)
* **WebAssembly tiles and third-party tiles** that draw their own picture are walked only as far as they register clickable regions
  with labels; unlabelled regions are skipped, and a tile that draws pixels without regions is silent. A tile in a window of another
  program (a Linux app) is outside all of this.
* **Braille is not supported** (no braille display driver, no braille output).
* No typing echo (characters, words) and no spell-out; text is read when a field takes the focus.
* The reader follows the focus of fields and chosen rows plus the walk. A tile that moves its own highlight without using the
  toolkit's row or field does not speak until it calls `announce` or `describe`.
* Dialogs of tiles are not announced by title yet, only the ones the shell itself owns (sign-in, lock, Help, power menu) and the messages
  Settings pages show. Other tiles' toasts are spoken because the shell's toast is.
* Volume and brightness changes show a bar and are not spoken.
* The large pointer applies on the framebuffer (a real screen). In a browser window the browser's pointer is the browser's.
* Reduced motion covers the moving backgrounds, the sign-in galaxy and the caret; some small spinners and progress animations in
  individual tiles still move.
* Slow keys also delay the volume and media keys.
* The recorded voice reads what it is given; it does not choose pitch or a different voice for roles.
* The page for narrow windows (under about 760 pixels) lists the switches in one column and may cut the last ones off; the keys
  `r n c p f m k w u` always work.

## Browser

The built-in Browser tile (the reader-style engine, `crates/iyakki-web`) now exposes the page itself, not only its address bar and
buttons. This section supersedes the gap "Web pages in the Browser tile are not exposed" above for pages shown by that engine. **No
screen reader user has tried it**; everything below was exercised with a fake voice and fake keys on three fixture pages (a shop, a
sign-in form, an article).

### What the page tells the reader

The page's tree is made when the page is: **roles** (link, button, heading with its level, list and list item "3 of 7", form field,
check box, radio button "2 of 3", drop-down, picture, table with caption, rows and columns, group, landmark: navigation, main, banner,
footer, complementary, search, a named region or form), **names** (`aria-labelledby`, `aria-label`, `label for`, a wrapping label, the
text inside, `alt`, `title`, a placeholder as the last resort) and **states** (checked, unavailable, expanded or collapsed, required,
invalid with the page's error message, read only, selected, pressed, current page). Not shown to the page's reader: `display:none`,
the `hidden` attribute, `aria-hidden="true"`, hidden inputs, a closed `details` (only its summary), pictures with `alt=""`.
Text that is only hidden by moving it off screen (the "screen reader only" trick) stays, because that is what it is for. A password
is never read back.

What is said: "Sign in, button", "Heading level 2, Pricing", "Email, edit text, required, invalid, Enter an email address with an at
sign", "Remember me, checkbox, checked", "Image, Red running shoe", "Sizes and prices, Table, 3 rows, 2 columns", "$55, column Price",
"Primary, navigation landmark". The words are made by one function (`iyakki_web::a11y::speak_node`) from the node and, for fields, the
value the person typed or chose, and handed to the operating system's screen reader above (`Role::Spoken` of the toolkit). **Nothing is
spoken unless the OS screen reader is on** (Super+Alt+S). When a page appears it says its title, the number of links, headings and
form fields, and any `role="alert"` text.

### Keys in the Browser

| Key | What it does |
|---|---|
| Tab, Shift+Tab | Next, previous link or control in page order, with a ring; `tabindex`, disabled controls and radio groups are handled like a browser does. Enter or Space presses it |
| Up, Down, Left, Right on a radio button | Choose the previous or next button of the group (they no longer go back or forward while a radio button has the focus) |
| V | **Browse mode** on or off (on by itself while the screen reader is on, see the settings below) |
| H, 1 to 6 | Next heading, next heading of that level (Shift: previous; `!` `@` `#` `$` `%` `^` for the levels backwards) |
| L, F, B, T, R, I, G | Next link, form field, button, table, landmark, list, picture (Shift: previous). Past the end it wraps and says so |
| M | Skip to the main part: the page's own "skip to content" link, else its main landmark |
| K, J, E, Z | A list of the links, headings, form fields, landmarks. Type to search (starts-with first, then contains), Up and Down choose, Enter goes there, Escape closes. The rows can be clicked |
| A | Read the article aloud (the reader view's text); A again pauses and continues; Esc stops |
| W | Read the whole page in order, menus and landmarks included |
| Ctrl+F, or `/` in browse mode | Find in the page |
| ? | Say what the page has (links, headings, landmarks, fields without a label, pictures without a description) |

In browse mode the letters above belong to the page: B, F, R, H, G no longer go back, forward, reload, home and to the address bar
(use the arrow keys for back and forward, Ctrl+L for the address bar), and capital F, B, R, T no longer start the full engine, the blocked
list, the reader view and translate. V turns it off again. A text field that has the focus takes the letters you type.

A link to `#something` on the same page moves to that place and says it. A form that breaks its own rules (an empty `required` field, an
email without an at sign, a value shorter than `minlength`, a `required` check box not checked) is **not sent**: the focus goes to the
first such field and the reader says why. A page that comes back after a send with fields marked `aria-invalid` puts the focus on the
first one and says how many there are. Pages that mark errors only with colour, or by script after the page loaded, are not understood.

### Looks

* **Text size** of Settings -> Display already scales the whole tile, the page text included; the per-site zoom (`+` `-` `0`) comes on top.
* **High contrast**: with the computer's High contrast theme (or "page colours: always" below) every colour the page chose is replaced by
  the OS palette: black ground, white text, yellow links, cyan visited links, white borders, links underlined, no backgrounds, no
  shadows. A test computes the ratios (every pair at least 7 to 1, the check refuses a palette below WCAG AA).
* **Minimum font size** (12 to 32 pixels at normal zoom; the page's body text is 16) raises smaller text.
* **Reduce motion** makes `@media (prefers-reduced-motion: reduce)` rules apply and stops animated pictures at their first frame. The
  engine has no CSS animations to stop.
* **Big focus ring** doubles the ring round the link or control that has the keyboard (forced colours do too, in the palette's yellow).

### Choices

Everything in the first list follows Settings -> Accessibility. The Browser's own choices are in `browser-a11y-<name>.json` and are
changed by the assistant: `browser_accessibility_status` (free; shows them and what the page in the tab has) and `browser_a11y_set`
(asks every time): browse mode (true, false, "follow" the screen reader), forced colours ("follow" the computer's High contrast, "always",
"never"), minimum font size, stop animations (true, false, "follow" Reduce motion). The OS choices are still changed with
`accessibility_set`; there is no second copy of them.

### What is not covered

* **Pages shown by the full web engine (F, WPE/WebKit)** are a picture drawn by a helper program: the tile exposes only its own bar
  and buttons. The helper (`dist/webview/wpe-helper/helper.c`) draws offscreen and passes frames, input and a title; it does not hand over
  the page's accessibility tree, so a screen reader hears nothing of such a page. A sighted helper sees the page and the OS text size
  does not reach it.
* The tree is built once, when the page loaded. The engine runs a page's scripts once for sites you allowed and keeps no live page after
  that, so content a script adds later, live regions that change, and errors a script shows after a click are not announced.
  `aria-live` text present at load is said once.
* Canvas, SVG pictures (even with `role="img"`), video and audio controls, iframes and plug-ins have no tree nodes. Pictures of text are read as their `alt` only.
* A `div` with only `tabindex` and no role, and custom widgets (tabs, menus, trees) without a role, are not stops. `role="button"`,
  `link`, `checkbox`, `radio`, `textbox`, `heading`, `img`, `list`, `listitem`, `tab`, `option` and `menuitem` are understood, and
  a `summary` is a stop that can be spoken but not opened (the page shows a closed `details` without its content).
* "Visited" is spoken nowhere; visited links differ only by colour in forced-colours mode.
* The ring round a heading, landmark or table is found by matching its words on the drawn page; repeated words are matched in order, but
  a page whose words are split by images or formatting oddly may get the ring a little off. What is said is always right; where the ring
  is drawn is a best effort.
* The pacing of "read page" is an estimate (about 15 characters a second), not the true end of each sentence; very slow voices fall behind
  and the reader queues up to eight sentences.
* Touch scrolling, pinch zoom and the on-screen keyboard (the rest of F-BROW-38) are not part of this.
