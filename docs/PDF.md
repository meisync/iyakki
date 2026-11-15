# PDF: reading, viewing and saving (F-BROW-35, F-PDF-01 to F-PDF-07)

IYAKKI reads and writes PDF with its own code (`crates/iyakki-pdf`, no dependencies except the browser engine's JPEG decoder).
This page says what works, what does not, and how to use it. It is not a full PDF engine.

## What you can do

| You want to | How |
|---|---|
| Read a PDF from the web | Open its address in the Browser. The page says it is a file. Press **O**: it is saved to Downloads and opens in the viewer. **D** only saves it. |
| Read a PDF you already have | The PDF viewer ([`PdfApp`](../crates/iyakki-shell/src/apps/pdfview.rs)) lists the PDFs in Downloads and opens one with a click; any tile can open a file in it with `open_item("file:<path>")`. The Browser opens saved files in the same viewer. |
| Move around | `n` / `p` or the arrows: next and previous page. Home and End: first and last. Page Down and Space scroll, then turn the page. The page list on the left jumps. |
| Zoom | `+` and `-` step 50% to 400%; `w` or **Fit width** fits the page to the window (the default). |
| Search | `/` or Ctrl+F opens the box; Enter searches every page in the background. The count reads "3 of 17" (with "searching" while it runs), matches are highlighted on the page (the current one orange), the page list shows matches per page. `g` and `G` (or **Next match** and **Prev match**) step through them. |
| Copy text | `c` copies the text of the page, `C` (capital) all of it, to the clipboard. A page with no text (a scan) says so. |
| See and fill form fields | A **Form (n)** button lists the fields and their values; click a row to fill it in (a text field opens a bar to type in, a check box flips, a choice goes to the next option). Changes are marked `*`; **Save copy (n)** writes `name (filled).pdf` next to the file, never over it. Radio buttons, push buttons and signatures cannot be filled (KI-715). |
| Save a page as a PDF | In the Browser press **Ctrl+P**: the page's reader version (headings, text, lists, tables as aligned text, the pictures that have arrived) is written to Downloads as a new PDF. **Ctrl+Shift+P** switches the page size between A4 (default) and Letter. |
| Ask the assistant | `pdf_info` (pages, title, size, form fields and which can be filled), `pdf_form_fill` (fills fields and saves a new file; asks first), `pdf_text` (the text of some pages, bounded by `max_chars`, marked untrusted), `page_save_pdf` (asks first) and `pdf_create` (see below). |
| Make a PDF from text | Ask the assistant: `pdf_create` takes a title and Markdown-ish text it wrote (`#` headings, paragraphs, `-` and `1.` lists, `>` quotes, fenced code, `---` rules, `\|` tables kept as aligned text, `![alt](path)` pictures from your folders or a `data:` address) and asks before it writes. Output: the saved path and the page count. Caps: 300 000 characters, 200 pages, 40 pictures of 3 MB. A file that exists is never replaced; a folder you may not write to is refused; text the writer cannot write (other scripts) is refused with the characters named; the text is only text to put on pages, never instructions. |
| A story, a plan, a quiz or a lesson as PDF | TALES: **Save PDF** (key `d`, or say "make it a PDF" in its conversation) writes `Stories/<title>/story.pdf` beside `story.md`, with the illustrations (the first one right under the title serves as the cover). LEARN: **Export PDF** on a plan, a quiz and a written lesson (key `p`) writes to Downloads; for a quiz **Answer key: off/on** (key `k`) puts the key on a page of its own, off by default and never offered while a quiz is being taken. The assistant does the same with `study_export_pdf`. |

## The full web engine and printing

The full engine is WPE WebKit (`dist/webview/wpe-helper/helper.c`). The WPE API has no print operation, and the helper has no
printing code, so a page shown by the full engine cannot be printed to PDF faithfully. Ctrl+P on such a page says so and points to
the built-in reader (Esc), whose version of the page is saved instead. A picture-perfect copy of a script-built page would need an
engine with print support; that is not available here and is not claimed. (This was checked in the helper source and the WPE
header it includes; it was not tried on a device.)

## What is supported when reading

* File structure: header, cross-reference tables and streams, object streams, incremental updates, hybrid files. A damaged file
  (cut off, wrong `startxref`, junk before the header) is rebuilt by scanning and the viewer says "damaged file, rebuilt".
* Page tree with inherited boxes and rotation; CropBox; cycles in the tree end.
* Streams: Flate (own decoder), LZW, ASCII85, ASCIIHex, RunLength, PNG and TIFF predictors. JPEG pictures use the browser's own JPEG
  decoder.
* Text: simple fonts (WinAnsi, MacRoman, Standard, `/Differences` with glyph names), Type 0 fonts through ToUnicode, widths,
  character and word spacing, `TJ` kerning. Lines are formed by baseline, spaces by gaps.
* Drawing: paths, fills (nonzero and even-odd), strokes with caps, joins and dashes, clipping, device/ICC/Lab/Indexed/Separation
  colour (a type 2 tint function), constant alpha, images of 1 to 16 bits, image masks, soft masks of images, inline images, form
  XObjects, annotation appearance streams (form fields, stamps). Text is drawn with the shell's own fonts (sans, bold, mono) at the
  PDF's positions and widths.

## What is not supported, and what the viewer shows instead

| Not supported | What happens |
|---|---|
| Password-protected files (RC4, AES; any password, even the empty one) | The file is refused with "password-protected PDFs are not supported yet". Nothing is shown. |
| Radio buttons, push buttons and signatures in forms (signatures are shown as their appearance only, not verified), JavaScript in PDFs (never run) | Text, choice and check box fields can be filled and saved as a copy; the others are listed read-only. |
| Pattern fills, shadings (gradients), soft-mask groups, blend modes | Patterns are drawn grey, shadings and soft-mask groups are skipped; the line under the page names what was left out. |
| JPEG 2000, JBIG2, CCITT fax pictures | A grey box in the picture's place and a note. |
| Embedded font programs (TrueType, CFF, Type 1) | The text is drawn with the shell's own font by its Unicode value, at the right place and width, so glyph shapes differ from the original. Text in a font with no Unicode map (many subset CID fonts without ToUnicode) is drawn as boxes and cannot be copied or searched. |
| Type 3 fonts' glyph drawings | Their text is extracted but drawn as boxes. |
| Rotated or slanted text | Drawn as a line along the baseline (extraction is not affected). |
| Optional content (layers) | All layers are shown. |
| Text recognition in scanned pages | None. A scan has no text to copy or search; the viewer says so. |
| Reading order of multi-column pages | Done (F-PDF-08): lines are cut at very wide gaps, the empty strips between columns are found, columns are read one after the other and a heading that crosses them keeps its place (`crates/iyakki-pdf/src/order.rs`). Text from fonts without a Unicode map is F-PDF-11. |

## What the writer makes

Pages are A4 or Letter, 0.75 inch margins, a footer with the title and page number. Headings (6 levels), paragraphs, list
items, quotes, preformatted code, rules and pictures. It uses the standard Helvetica and Courier fonts for Windows-1252 text. A block with other characters is drawn with an **embedded subset of DejaVu Sans, Bold or Mono** (F-PDF-11): Greek, Cyrillic, Latin Extended, Armenian, Georgian, arrows, math and other symbols, as a Type 0 font with a ToUnicode map, so the text can be copied and found. Scripts that need shaping or right-to-left layout (Tamil, Devanagari and the other Indic scripts, Hebrew, Arabic, Thai) and Chinese, Japanese and Korean cannot be drawn correctly yet (F-PDF-15): the assistant's tools and the TALES and LEARN buttons refuse them with the script named and write nothing; the writer itself, called directly, writes them as `?` and counts them. Page text and pictures are compressed with an in-house Flate encoder (LZ77 with the fixed Huffman table, `iyakki-pdf::filter::zlib_deflate`, since alpha.3); pictures are RGB, with a soft mask when they have
transparency.

## When the assistant offers `pdf_create`

`pdf_create` is offered to every model that does not produce PDF files itself, which is every provider and model by default. A provider that really does (`native_pdf_output: true` in its provider settings) or an external agent such as Claude Code that has its own file tools (`native_pdf_output` in its agent settings) is not offered the tool; its other PDF tools stay. The field is additive and false when absent. `pdf_create` is in the same tool group as the other PDF tools and the PDF prompt rule says so. The switch is key **P** on Settings -> Models (the chosen provider) and key **p** on Settings -> Agents (the chosen agent), both for administrators, and the assistant's `pdf_output_status` (free) and `pdf_output_set` (asks every time) change the same setting.

## Safety

A PDF is untrusted input. The reader caps object nesting (48), page tree depth (40), form nesting (10), decoded bytes per file
(384 MiB total, 96 MiB per stream), operators per page (4 million), page pixels (36 million), image pixels (40 million) and path
size, ignores cycles, never runs anything in the file, and works on a separate thread in the viewer so a hostile file cannot
freeze the screen. Tests mutate small PDFs thousands of times and feed deep nesting, self-including forms and a decompression bomb;
none may panic or hang. `pdf_text` output is marked untrusted for the assistant. The tile reads files only where the person's
file rules allow; saving never overwrites a file (a numbered name is used).

## Tests

`cargo test -p iyakki-pdf` (reader, renderer, writer; PDFs are built inside the tests, nothing is committed),
`cargo test -p iyakki-agent --lib pdftools` (chat tools, `pdf_create`), `cargo test -p iyakki-agent --lib storypdf studypdf` (TALES and LEARN files), `cargo test -p iyakki-shell pdfview` and the browser test
`a_pdf_file_page_opens_in_the_viewer_and_a_page_is_saved_as_pdf`.

## Not done yet

* A launcher tile for the viewer: `PdfApp` is a complete `App`, but adding it to the launcher means a new app id in the policy,
  roles, cloud mode and memory gate (`iyakki-sec/src/policy.rs`, `cloudmode.rs`, `memgate.rs`, `shell.rs`), which other work in
  progress was changing; it was left for one pass.
* A chat tool that opens the viewer on a file.
* Office files (the other half of F-BROW-35) are the media/office package's job (F-MEDIA-01).
* Encryption, embedded font rendering, printing to a printer (see the printing work).

## Passwords (F-BROX-07)

Files protected with the standard security handler open with their password: RC4 (40 to 128 bit) and AES-128 (`iyakki-pdf::crypt`, written from RFC 1321, RC4 and FIPS 197). The viewer asks in its own page (masked field; the password goes to the reading thread only and is not kept); the empty password, used by files that are only "owner protected", opens without asking. The assistant's PDF tools say that a file needs a password and where to type it; there is no password argument, so a secret never passes through the model. AES-256 (revisions 5 and 6) is refused in words (KI-710); print and copy limits are not enforced.

## Checking filled forms with another reader

The form writer (`iyakki-pdf::form`) is also checked against a reader that is not ours. On a Mac, `crates/iyakki-pdf/tests/data/check-forms.swift`
opens the files the ignored test `write_filled_samples_for_other_readers` writes and prints each field as Apple PDFKit reads it (the
expected values are in the file's header). On 2026-11-13 both the table and the cross-reference-stream variants read back correctly,
including the checked box. Other readers have not been tried.
