# Multiple languages: plan for alpha.4

Status: plan, 2026-11-11 (owner: "add multiple languages support on alpha.4"). Nothing is built. Today every screen string is hard-coded English across the shell and
agent; what exists is **font coverage** (Noto Sans for Devanagari, Bengali, Gurmukhi, Gujarati, Tamil, Telugu, Kannada, Malayalam, Arabic and Hebrew, with shaping) and the
contract's stable error codes and message keys (CORE-CONTRACT.md). Compatibility rules: [CORE-COMPATIBILITY-DESIGN.md](CORE-COMPATIBILITY-DESIGN.md).

## 0. Scope decision (owner, 2026-11-11)

**Alpha.4 adds the support, not the translations.** We ship the framework so a person or community can **build their own language** (screen text, formats, fonts, keyboard and **voice**) as an installable
pack, with the tools to do it and test it. No machine translation: people write their own translations. We ship English complete, plus a pseudo-locale and one small **sample language pack** that shows how a pack is made. Which real languages we translate ourselves, and who
reviews them, is decided later; nothing in alpha.4 waits on it. **Voice is in scope for alpha.4** as the same kind of pack (section 2, "Voice packs").

## 1. Goals

1. A person picks their language (per account, with a system default); the screen, the browser client, chat tool results and error messages follow it.
2. Adding a language needs **no code change**: a catalogue file plus (if needed) a font. Community catalogues install like any other package.
3. English stays complete and is the fallback for any missing key, so a half-translated language is usable and never shows a blank or a key name.
4. Layout never breaks: longer words, right-to-left scripts, larger fonts. The existing overflow monkey tests run on a pseudo-locale.

## 2. Design

* **Message keys, not English sentences, in code.** `tr("settings.sound.volume")` with the English text in the base catalogue. Keys are stable and **immutable** (a changed meaning gets a new key,
  CORE-COMPATIBILITY-DESIGN.md rule 5). Error codes already are stable strings; their texts move into the catalogue.
* **Catalogue format:** one JSON file per language and per component (`core`, `shell`, each app, each bundle), `{ "catalogue_version": 1, "lang": "hi", "entries": {key: text-or-plural-forms} }`, versioned and
  with unknown fields kept. Plain data, loaded at run time, no code in a catalogue.
* **Formatting:** a tiny in-house message syntax (named placeholders, plural categories per CLDR rules for the languages we support, a `select` for gender or role) and in-house number, date, time
  and size formatting by locale (extend `clock::format` and the size/number helpers); no new dependency. Locale data tables are small generated files kept in the repo.
* **Selection order:** person's language, then system language, then English, per key. A catalogue may declare `fallback: "hi"` for a regional variant.
* **Right-to-left:** the drawing layer already shapes Arabic and Hebrew; add paragraph direction, mirrored layout for rows, back/forward arrows, and scrollbars, tested at the four sizes.
* **Input:** keyboard layouts per language (the keyboard work already has layout tables), compose and dead keys; an input-method step for Indic and CJK is a separate item (open question 3).
* **Fonts:** keep Noto where it exists; CJK needs a subset or a download (size: Noto Sans CJK is large). Fonts come from the language pack, not the base image, to keep the image small.
* **Language packs are packages:** kind `language` in the Store (and a member kind in bundles, CORE-ACCESS-MODEL 6d), signed like any package, installable on bare metal and cloud, with `requires` the catalogue version.
  An app or bundle ships its own catalogues next to its code. Wizards (6e) and browser extensions take their texts from catalogues too.
* **Clients:** the instance sends **keys and arguments** over the session protocol where the client draws (display-list mode later); in picture mode the instance renders in the person's language. The
  browser client's own chrome (connect page) carries a small built-in catalogue set.
* **Assistant and voice:** the chat agent answers in the person's language by default (a setting); tool results keep English keys for the model and show the translated text to the person; the wake phrase
  and speech recognition per language are separate (open question 4).

### Voice configuration comes with the language pack (in scope for alpha.4)

* A language pack **initialises with the model configuration for speech-to-text and text-to-speech** for its language. The pack carries **configuration, not models**: for each of
  `stt` and `tts` it names the engine kind (local model file, a configured provider, or none), the model identifier or file name to look for, the language code, sample rate and any
  engine options, and the **wake phrase and command phrases** as data. The base image and the pack ship no models (permissive-only rule: a model is downloaded or supplied at the person's request).
* `iyakki lang new <code>` writes this configuration skeleton (`voice.json`) next to the catalogue, with empty model references for the author to fill. People can point it at whatever models they use.
* **On install, the pack runs its setup wizard** (the shared wizard engine, CORE-ACCESS-MODEL 6e): it shows what the language needs, lets the person choose or add the STT and TTS models (a file on the machine,
  a provider already configured in Settings, or a download they confirm), tests them with a short record-and-play check, and saves the choice. Skipping is allowed: the language then works for text and voice
  falls back to English with an on-screen note ("no speech-to-text model for <language>, using English").
* The core's voice session (MY-IYAKKI-PLAN.md) reads the person's language, loads the pack's configuration and the models it points to, and uses the same engine interface for every language, so
  adding a language never needs engine code.
* All audio rules stay: models run locally by default, the microphone is a hard-ask capability with a card on each use, the wake phrase is **experimental** and off by default (owner, 2026-11-10), and a pack
  contains no code.
* The authoring kit validates the voice configuration (fields, language code, that referenced files exist when given) and the record-your-phrases helper in Settings tests the match rate on the person's voice.

## 3. Work, in order

1. **Inventory and extraction tool.** Find every user-visible string (draw calls, form labels, hints, errors) with a script; produce the base English catalogue and a report of strings that
   are built by concatenation (these need rewriting into placeholders).
2. **The engine:** `iyakki-i18n` (pure Rust, no dependencies, wasm32-safe): catalogue load, fallback, plurals, formatting, pseudo-locale generator (accented, +40 % length, brackets, and a RTL mirror mode).
3. **Wire one area end to end** (Settings, Sound, plus the sign-in screen and the error texts) to prove the path, with golden tests, overflow tests on the pseudo-locale, and a language picker.
4. **Convert the rest** tile by tile (the compile-split crates make this parallel), enforced by a ratchet gate that counts hard-coded strings and only decreases.
5. **Language packs in the Store** and a contribution guide (how to translate, how to test with the pseudo-locale, licence: translations are Apache-2.0 like the code).
6. **Authoring kit instead of first languages:** `iyakki lang new <code>` (copies the English catalogue with every key and its placeholders), `iyakki lang check <dir>` (missing keys, placeholder and plural mismatches, overflow run on the pseudo-locale, font coverage of the characters used), `iyakki lang build <dir> -o pack.iyp` (signed with the person's own key, trust shown on the install card like any package), and a guide. One tiny sample pack proves the path end to end; real languages come from people using the kit.
7. **Docs and screenshots** per language where they differ (the docs revisit in alpha.3 already makes screenshots regenerable).

## 4. Tests and gates

* Pseudo-locale overflow run over every tile and size (the existing monkey tests), RTL mirror run.
* Missing-key test: every key used in code exists in the English catalogue; every other catalogue only has keys that exist (stale keys are reported, not fatal).
* Placeholder test: a translation must use the same placeholders as English; plural forms must cover the language's categories.
* Golden formatting tests for numbers, dates, sizes and plurals per language.
* Compatibility: catalogue version and key immutability are in the compat gates (charter section 8).

## 5. Open questions

2. Input methods for Chinese, Japanese, Korean and Indic phonetic input: in alpha.4 or later (proposal: a separate item; packs may declare a keyboard layout, not an input method)?
3. Which speech-recognition and text-to-speech engines the voice packs target first (a permissively licensed local engine; to be chosen when the voice work starts)?
4. Should the instance or the client choose the language when they differ (proposal: the person's account setting wins; a client may override for its own chrome only)?
