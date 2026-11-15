# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

> Schema note: the four allowed values are `web`, `ios`, `android`, `adaptive`, and none of them is true here.
> IYAKKI's shell is **not a browser and has no DOM, CSS or HTML**: every pixel is drawn by in-house Rust code
> (`tiny-skia` for 2D, `fontdue` + `rustybuzz` for text) into a Linux framebuffer, a desktop window (eframe), or a
> canvas streamed to a browser. `web` is recorded only because the schema admits no other value; design work must
> never emit HTML/CSS as the deliverable. See `## Capabilities and Constraints`.

## Stack

Existing codebase. Rust, 79 crates, one static musl binary. UI: `crates/iyakki-uikit` (design system: `fx.rs`,
`theme.rs`, `card.rs`, `gfx.rs`) and `crates/iyakki-shell` (launcher, 37 tiles under `src/apps/`). Separate
workspace `crates/iyakki-gui` for the desktop window (eframe). No web framework, no CSS, no component library.

## Users

Confirmed by the user: **both audiences at once, held to the stricter bar.**

- **Operators / platform teams** — the literal comparison the user drew is Argo CD. They live in consoles, scan
  for state, and judge this UI against Grafana, Argo CD and Datadog. Density, scanability and at-a-glance status
  outrank expression.
- **Developers and testers** — the alpha audience `README.md` names. They explore, try features and file issues,
  so each tile still has to explain what it is.
- Four roles exist in code and gate every tile: admin, operator, user, guest (`iyakki-sec/src/policy.rs`).

## Product Purpose

An operating system built around an AI assistant that is allowed to *act* — read files, call APIs, run commands,
start containers — without being trusted blindly. The shell is the whole product surface: sign-in, a launcher of
37 role-gated tiles, and the apps themselves.

## Positioning

One permission model in front of every tool, whoever invokes it. Reading is free; anything risky pauses for
*Allow once / Allow for this session / Deny*; a short always-ask list cannot be bypassed even in Autopilot; every
run lands in a hash-chained ledger. Untrusted code runs in a wasmtime sandbox with fuel, memory and wall-clock
caps. A neighbouring product cannot truthfully copy this because it is enforced below the apps, not inside them.

## Operating Context

The same binary runs in four places, and the UI must hold in all of them: bare metal as PID 1 on a framebuffer;
a QEMU VM; a desktop window; and streamed to a browser as pixels (`iyakki-remote`). The live reference device is
a 1920×1080 machine at 192.168.1.20, paired over developer mode. `./run.sh screens` renders every screen to PNG
at 1280×720 for review.

## Capabilities and Constraints

- **Renderer**: `tiny-skia` software 2D. Rounded rectangles, strokes, gradients and images are cheap. **There is
  no blur primitive** — macOS-style vibrancy/frosted glass must be approximated with flat or gradient fills, not
  a real blur.
- **Text**: `fontdue` + `rustybuzz`. Bundled faces are Noto Sans (Latin + Devanagari, Bengali, Tamil, Telugu,
  Kannada, Malayalam, Gurmukhi, Gujarati, Arabic, Hebrew) and **DejaVu Sans + DejaVu Sans Mono**. SF Pro cannot
  ship (proprietary). CJK and emoji render as empty boxes today.
- **Scale**: `shell::scale_for` = `0.9 × (window / 1280×720)^0.3 × text size` — 0.9 at 720p, ~1.0 at 1080p, 1.25
  at 4K. No text is drawn below 10 px.
- **Themes already exist**: `ThemeId::{Dark, Light, HighContrast}` in `iyakki-uikit/src/theme.rs`. `Gfx` is the
  single place a colour becomes pixels and it consults `themed()` first, so a palette change propagates to every
  screen without touching the apps.
- **Licensing**: Apache-2.0, permissive dependencies only, enforced by `cargo deny`. Any new font or asset must
  clear that bar and is weighed against image size (the libre ISO is ~60 MB).
- **Boundary**: `BOUNDARY.md` lists every touchpoint with the base OS; new ones need a row in the same change.
- **Device matrix for this work** (user-confirmed): 1920×1080 with mouse and keyboard is the target. The
  remaining ranges (1280×720, 4K, touch, browser front end) are delegated — "do what is recommended and best" —
  so the design should degrade sanely across `scale_for` without those being acceptance criteria.

## Brand Commitments

- Name **IYAKKI**; logo at `crates/iyakki-shell/assets/logo.png`.
- **Binding visual constraint volunteered by the user**: imitate macOS — "follow the exact macOS UI standard" —
  but in **a different style**, with **different default app icons**. Recorded as given, not expanded.
- The incumbent dark cyan/magenta neon world (`docs/DESIGN.md`) is explicitly rejected by the user as generic and
  boring. Under the redesign rule it is evidence and anti-reference, not a starting point.
- Every change must be previewable as an image before any code is written — the user reviews and decides first.

## Evidence on Hand

- `FEATURES.md` — 1426 rows, 425 done / 913 partial / 57 planned, verified by a test.
- `KNOWN-ISSUES.md` — 79 numbered issues; the honest hardware record.
- `docs/DESIGN.md` — the incumbent visual system, documented in full.
- Live device screenshot at `/tmp/iyakki-device.png`; first light mock at `/tmp/iyakki-light-mock.png`.
- Real tile inventory: 37 tiles in 5 groups (Ask & Create 7, Build 9, Media 6, Data 3, Devices & System 12),
  from `const TILES` in `crates/iyakki-shell/src/shell.rs`.
- No customers, benchmarks, pricing or deployment claims exist. Future work must not invent them.

## Product Principles

1. **State belongs on the surface.** An operations console answers questions before the first click; a launcher
   that shows only names wastes the screen.
2. **Permission is visible, never implied.** Role locks, approvals and audit are product truth and must be
   legible in the UI, not hidden.
3. **Honesty about maturity.** Most features are partial; the UI must be able to say "needs setup" or "not
   verified" without shame.
4. **The renderer is the budget.** A design that cannot be drawn by `tiny-skia` at 30 Hz on a 2018 Mac mini is
   not a design for this product.
5. **One language across 37 tiles.** A change to the shared system must land everywhere, not decorate one screen.

## Accessibility & Inclusion

- A `HighContrast` theme ships and must keep working: all text ≥ 7:1, visible borders, thicker focus ring.
- `fx::readable(fg, bg, min)` enforces ≥ 4.5:1 for text on cards, and a test checks the grey ramp.
- `IYAKKI_REDUCED_MOTION=1` freezes animation; honour it in anything new.
- Text size is per person (Settings → Account), applied through the scale factor.

## Open Decisions

- **Theme scope is undecided.** The user was asked whether Light becomes the default, whether Dark keeps parity,
  or whether Light is a launcher-only pilot, and answered with the macOS direction instead. Recorded as open;
  the comps assume Light as the surface under review, which is not the same as a decision that Dark is retired.
