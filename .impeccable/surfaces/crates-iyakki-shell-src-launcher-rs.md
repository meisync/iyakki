---
version: 1
slug: "crates-iyakki-shell-src-launcher-rs"
primary_target: "crates/iyakki-shell/src/launcher.rs"
related_targets: ["crates/iyakki-uikit/src/theme.rs","crates/iyakki-uikit/src/fx.rs"]
---

Scope: the IYAKKI home screen (launcher) only — `crates/iyakki-shell/src/launcher.rs`, drawn by `iyakki-uikit`
(`fx.rs`, `theme.rs`, `card.rs`, `gfx.rs`) into a framebuffer at 1920×1080. Visitor mode: Operate.
Audience: platform operators and developers on one machine, held to the operator bar. Job: reach the right one of
37 role-gated apps, and see what the machine is doing, without hunting. Constraints: tiny-skia (no blur), bundled
fonts only, `scale_for` 720p→4K, three themes must keep working, Light is the surface under review.

## Direction contract

THESIS: the home screen answers "what is this machine doing right now" before it offers a way in. It refuses the
paged grid of same-size tiles — the category default this launcher ships today — and refuses a wall of 37 cards.

OWN-WORLD: engraved score. Warm paper #F4F0E6, sheet #FDFBF6, ink #16181C, hairline #D8D1C2, and the two pencils a
conductor marks with — blue #1D4E89 (selection, running) and red #B3322C (critical) — with ochre #8A6212 degraded
and ink-green #2F6B4F healthy. Inter for UI, an italic serif for the single editorial voice, bundled DejaVu Sans
Mono for every figure, tabular. Custom squircle icons, ink glyphs on state-tinted grounds. macOS supplies topology
and controls only: menu bar, Dock with running dots, standard fields and lists.

STORY: the operator sees the machine's whole state in one line, types what they want, and acts — approving the one
thing that needs them rather than browsing an app grid.

FIRST VIEWPORT: menu bar at 26 px carrying model, cluster, runs, clock, account. A 760 px command field centred at
the optical third, focused, placeholder "ask, open an app, or run an action", results grouped ACTIONS then APPS,
the primary row filled blue with ↵. Beneath it, full width, the signature move: all 37 apps as engraved marks on
one stave, bar lines dividing the five groups, group names below. Beneath that, four cards for what needs a person.
Dock bottom-centre. Primary action is the command field; the stave is read, not clicked first.

FORM: command-first home, position 7 of 7 on the ordered grounded list. seed:a668c98f6f7990f8

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Unresolved

- Whether Dark keeps parity with Light, or Light becomes the default (user asked but answered on another axis).
- Live state per tile needs a `status()` on the `App` trait plus 37 implementations; without it the stave and the
  cards have nothing true to draw. This is the build's largest cost and is not yet scheduled.
- Comp C's lower third is deliberately quiet; if it reads as unfinished on a real 1080p panel, the attention cards
  grow rather than new matter being invented.
