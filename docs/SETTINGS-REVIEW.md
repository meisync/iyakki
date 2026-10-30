# Settings review

A review of every page of Settings (`crates/iyakki-shell/src/apps/settings*.rs`), made on 2026-10-23 at the owner's request:
"review all the settings, make it world class" and "let us keep it simpler, same for firewall". The aim: a power user keeps every
control, an ordinary person sees plain words, few choices up front, safe defaults and the difficult things folded away.

How it was done: every page's code was read, then each page was drawn by the test harness at 360x740, 800x480, 1280x720,
1920x1080 and 2560x1600 (`settings_ia_tests.rs`: every hit rectangle must lie inside the window, the page list's rows never
overlap, for an administrator, a user and a guest) and walked with the keyboard alone. Changes are listed per page; what was not
done is in the backlog at the end, in priority order.

## What changed, in order of user impact

| # | Change | Why |
|---|---|---|
| 1 | **Firewall: a simple view first** (Standard / Open / Locked down, "Allowed on this computer" with Add and Remove, Advanced folded away) | The page was a rule editor with single-letter keys; most people only need "is it on, and what did I open". See [FIREWALL.md](FIREWALL.md#the-simple-view). |
| 2 | **Overview start page** with "Needs attention" (firewall open, no AI model, clock wrong, Bluetooth firmware missing, data only in memory), each with a **Fix it** button, and "Common things" buttons | People did not know where to look; the list of 30 pages was the only map. |
| 3 | **Eight headings instead of five**, in a fixed order, with the pages under them: Overview, Account & people, Apps & AI, Network & sharing, Display & sound, Devices, System, Developer | "Security" held a firewall next to Git keys; Time and Account were last; Home layout was under Apps. |
| 4 | **Search by plain words and synonyms**: wifi, password, dark, font, sleep, mouse, backup, reboot, security, assistant... and a helpful empty state | "dark" and "font" found nothing. |
| 5 | **Account page in plain words** (no "instructions per run"), and its text never runs into the text-size card at small sizes | Jargon; overlap at 800x480. |
| 6 | **One warning colour** (`fx::AMBER`) instead of a hard-coded `rgb(255, 196, 94)` repeated in ten places in Settings | Theme readiness. |
| 7 | **Four layout bugs the new draw test found, fixed**: the text-size card's buttons ran off the left edge of a 360 px window; the button bar reserved a fixed 1 to 4 rows, so on Models and Dev cluster at 360 px the last buttons were drawn below the window (the bar now measures its rows); heading names in the page list ran out of their box ("ACCOUNT & PEOPLE" at 360 px); the page list did not follow the left and right arrow keys (it now scrolls to the page that is shown) | Found by drawing every page at five sizes. |
| 8 | **Tests** for all of the above, including a monkey run per Settings page | See the list below. |

## The new arrangement

| Heading | Pages (in order) |
|---|---|
| Overview | Overview |
| Account & people | Account, Vault, Secret providers |
| Apps & AI | Models, Agents, Skills, RAG, Voice, Browser, Host access |
| Network & sharing | Network, Firewall, Shared folders |
| Display & sound | Display, Displays, Home layout, Sound, System sounds |
| Devices | Bluetooth, Keyboard |
| System | Time, Host name, Memory, Storage, Install to disk, Cloud mode |
| Developer | Developer mode, Dev cluster, Git and SSH keys |

Every page is still in the list (the left/right arrow keys walk them in this order; a test checks that the keyboard alone reaches
all of them). `Tab::ORDER` is the one place that fixes the order; a test fails if a page is missing from it. Pages a role does not
have are simply absent, as before.

## Page by page

Roles: **A** administrators, **all** every signed-in account (a user, operator or guest sees only what their policy allows).
"Changed" is what this review did; "Deferred" goes to the backlog.

| Page | What it does | Who | Problems found | Changed | Deferred |
|---|---|---|---|---|---|
| Overview | Start page: what needs attention, common places | all (entries only for pages the role has) | did not exist | **new** | more checks (no backup feature exists yet to check; update status; low disk space) |
| Account | Who you are, password, text size | all | "Compute 1000000 instructions per run" is jargon; text could run into the size card at 800x480 | plain words, text cut before the card | show a "last sign-in" line; sign out button |
| Vault | Encrypted secrets (system vault for A, personal for all) | A and all | "System / personal" switch is a key (Tab); "Reset vault" wording is scary without saying what is lost | moved under Account & people | a visible System / Personal switch; say what Reset removes before asking |
| Secret providers | External vaults (HashiCorp) | A | jargon title | moved under Account & people | rename to "External vaults"; fold under Vault |
| Models | AI services and which one answers | A edit, all test | 12 buttons in two rows; "Embeddings", "Agent price" are jargon; no first-run guidance beyond the new Overview item | Overview points here when no model is chosen | a "Get started" card (one provider type, one key, one test) and folding the rest under "More" |
| Agents | Claude Code, Copilot and other coding agents | A | 13 buttons; "Auto" and "Use for new chats" need explaining | none | same folding |
| Skills | Reusable instructions | policy | typed Markdown in a form (Instructions field) | none | a larger text area |
| RAG | Document search tuning | all | the name "RAG" means nothing to most people | keywords "search knowledge documents" already find it | rename "Documents" with RAG in the blurb |
| Voice | Speech to text, text to speech, tests | A, or policy.voice | one very long page with 10+ buttons; engine names | none | split "Basics" and "Engines" |
| Browser | What the web browser may do | all | switches: fine | none | none |
| Host access | Folders of the host computer a window may use | all | empty in a real machine, with a reason; fine | none | none |
| Network | Ethernet and WiFi, join, private MAC | A | "Private MAC" in the main button row | none | fold MAC under "More" |
| Firewall | See [FIREWALL.md](FIREWALL.md) | A | rule editor first; 14 key-only actions; typed on/off; "CIDR" | **simple view first**, Advanced folded; Open asks first; removing SSH / Remote desktop asks; Locked down is shown and honestly refused; custom port and pasted address become the right rule | outgoing filtering (the engine does not have it), per-rule counters |
| Shared folders | Share a folder over HTTP | A, IDE/Edit users | needs the firewall's Shared folders preset (it already says so and offers `f`) | Add now exists as "File sharing" in the firewall simple view | a "Fix the firewall" button instead of a key |
| Display | Brightness, text size | A | text size was only found by knowing the word "UI size" | keywords font, bigger, smaller, text size, zoom | none |
| Displays | Several displays | A | many single-letter actions | none | fewer buttons, a picture-first layout |
| Home layout | Tile groups on the home screen | all | under "Apps" | moved to Display & sound | none |
| Sound | Output, volume, test | A | "Advanced" is a key letter A | none | none |
| System sounds | Chimes | A | fine | none | none |
| Bluetooth | The adapter | A | missing-firmware message (another change is making it honest) | shown on Overview when firmware is missing | none |
| Keyboard | Function keys, repeat, layout | A | Apple-specific wording on a PC | none | hide Apple controls when there is no Apple keyboard |
| Time | Clock, zone, network time | all see, A changes | 10+ rows of text; "ntp" jargon in the keywords only | Overview flags a clock that "looks wrong" | an "Automatic" switch that covers zone and network time together |
| Host name | The computer's name | A | rules for letters shown only after a refusal | moved under System | show the rules before typing |
| Memory | RAM, compressed memory, swap | A | zram, zswap, swappiness, typed values ("auto, off, 4G, 50%") | none (being edited by the boot-speed change) | a three-way choice "Automatic / Off / Custom" with Custom under Advanced; hide swappiness |
| Storage | Where data is kept, make a data disk | A | "Make data disk" erases: it already asks to type the disk name and ERASE | Overview flags data kept only in memory | a plain "Your data is not saved" banner with one button |
| Install to disk | Put IYAKKI on the internal disk | A, real machine | erases: typed confirmation exists | none | none |
| Cloud mode | Hide pages that need a screen | A | jargon title for ordinary people | moved under System | rename "Server mode" |
| Developer mode | Pair a computer to push apps | A | off by default; fine | moved under Developer | none |
| Dev cluster | Clusters the AI IDE deploys to | when the IDE or k8s apps are allowed | 11 buttons | moved under Developer | fold |
| Git and SSH keys | Keys and tokens for git | IDE users | fine; typed host names | moved under Developer | pick the host from the list of known ones |

## Cross-cutting findings

* **Keys instead of controls.** Most pages are driven by single letters, with a hint line of 10 to 20 of them and buttons that send the same
  letters. Mouse and touch users have the buttons; keyboard users have the letters; both work. The weak point is wording that says
  "press k": it is meaningless to a touch user. The firewall's simple view uses button names in its sentences. **Deferred:** the same on the
  other pages.
* **Typed values where a choice would do.** Found: Memory (size, compressor, swappiness), Time server, Host name rules, the
  rule form of the firewall (Advanced). Paths already use the file picker (`Form::path`). **Deferred** except the firewall.
* **Risky actions.** Erasing a disk, resetting the vault, deleting a share and the dev cluster all ask for a typed word. New: the firewall
  asks before Open and before removing SSH or Remote desktop, and every firewall change keeps its 60-second undo.
* **"What happens next" text.** Present on the firewall, Memory, Displays and Install; missing on Models and Voice after a change.
* **Empty states.** Fixed for the page list's search. Good already on Storage, Host access, Shares. Missing a "do this next" on Agents.
* **Errors that say how to fix.** The firewall's new errors all end with what to check ("Check the port and try again", "Choose Discard
  to go back"). Others vary; a sweep is in the backlog.
* **Accessibility.** Every page is reachable with the arrow keys; `/` or Ctrl+F searches; Esc leaves. Contrast uses the shared colours
  (on the dark cards MUTED is 6.9 to 8.3:1 and DIM 4.7 to 5.6:1, all above 4.5:1; DIM is used only for hints). Text size has five steps (Account, Display). Row and
  button heights are at least 2.1 x the text size. **Deferred:** a focus ring on the buttons for keyboard users (today the hint line is the guide).
* **Scrolling.** Page list: wheel, trackpad and the bar. Firewall list: wheel moves the selection and the list follows. **Deferred:** move
  the long lists of Models, Agents and Storage to the shared scrolling list (`scrolllist.rs`).
* **Theme readiness.** The ten copies of the amber warning colour now use `fx::AMBER`; the page list background in `settings_nav.rs`
  still uses a literal `rgb(16, 21, 40)` and the vault/page cards use `rgb(60, 72, 110)`: **deferred** until the theme work names them.

## Tests added or changed

Added: `settings_ia_tests.rs` (headings and order, synonyms, the Overview and its attention list, role gating for administrator, user and
guest, every page drawn inside the window at five sizes with no overlapping rows, keyboard-only walk and search), the firewall simple view
tests at the end of `settings_firewall_tests.rs` (modes, Add, custom port, pasted address, the questions, the 60-second undo, drawn at five
sizes), `monkey_settings_every_page` in `monkey_tests.rs` (every page, three sizes, seeded random clicks, wheel and keys), and in `iyakki-fw`
the Web server preset (golden file `presets_all` regenerated on purpose: it now ends with the web server rules).

Changed on purpose (all because a page moved, or Settings no longer opens on Models): the agents, displays and system-sounds tests now expect
the new heading names and "Agents right after Models"; the tests that drive the Models page (`admin_adds_ollama_tests_it_and_picks_a_model`,
`api_keys_go_to_the_vault_never_to_settings_or_logs`, `a_rejected_form_stores_nothing`, `admin_sets_the_token_price_and_bad_values_are_rejected`,
`non_admins_cannot_change_system_settings_and_it_is_audited` with its new page list for a user, and the Lemonade tests) open it with
`SettingsApp::on(Tab::Models)` (a test-only helper); `every_settings_tab_is_reachable_by_clicking` first sees the Overview and then clicks
Models; the page-list test draws a taller window because the list now has eight headings; the Lemonade search test compares the header's
count with the chips that fit, because the button bar now reserves every row it needs and so a few fewer chips fit at 1920x1080.
Also: the firewall tests now run the **Advanced** page (`app()` sets `fw.advanced = true`; the page opens in the simple view
by default); the search test's "devices" group now finds two pages, not four (Network moved to Network & sharing).

## Backlog (highest impact first)

1. **Models and Agents "Get started"**: one card with provider choice, key, test; the 12-button row under "More".
2. **Memory in three choices** (Automatic, Off, Custom), swappiness and compressor under Advanced.
3. **Words for keys**: replace "press k" in page text by the name of the button, page by page.
4. **A visible focus ring** and a keyboard hint overlay (F1 already lists keys).
5. **Voice** split into Basics and Engines; **Network** MAC under More.
6. **Rename**: RAG to Documents, Cloud mode to Server mode, Secret providers to External vaults.
7. **Overview checks**: low disk space, an update waiting, a weak default password, no backup (needs a backup feature first).
8. **Shared scrolling list** for Models, Agents, Storage; theme constants for the left list.
9. **Firewall**: outgoing filtering to make "Locked down" real (engine work: an OUTPUT chain, a default for DNS, and an allow list UI).
