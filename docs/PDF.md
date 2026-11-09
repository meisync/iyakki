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
| See form fields | A **Form (n)** button lists the fields and their values. They are shown, not editable. The look of each field (its appearance) is drawn on the page. |
| Save a page as a PDF | In the Browser press **Ctrl+P**: the page's reader version (headings, text, lists, tables as aligned text, the pictures that have arrived) is written to Downloads as a new PDF. **Ctrl+Shift+P** switches the page size between A4 (default) and Letter. |
| Ask the assistant | `pdf_info` (pages, title, size, form fields), `pdf_text` (the text of some pages, bounded by `max_chars`, marked untrusted) and `page_save_pdf` (asks first). |

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
| Editing form fields, signatures (shown as their appearance only, not verified), JavaScript in PDFs (never run) | Fields are listed read-only. |
| Pattern fills, shadings (gradients), soft-mask groups, blend modes | Patterns are drawn grey, shadings and soft-mask groups are skipped; the line under the page names what was left out. |
| JPEG 2000, JBIG2, CCITT fax pictures | A grey box in the picture's place and a note. |
| Embedded font programs (TrueType, CFF, Type 1) | The text is drawn with the shell's own font by its Unicode value, at the right place and width, so glyph shapes differ from the original. Text in a font with no Unicode map (many subset CID fonts without ToUnicode) is drawn as boxes and cannot be copied or searched. |
| Type 3 fonts' glyph drawings | Their text is extracted but drawn as boxes. |
| Rotated or slanted text | Drawn as a line along the baseline (extraction is not affected). |
| Optional content (layers) | All layers are shown. |
| Text recognition in scanned pages | None. A scan has no text to copy or search; the viewer says so. |
| Reading order of multi-column pages | Text comes in the order the file draws it. |

## What the writer makes

Pages are A4 or Letter, 0.75 inch margins, a footer with the title and page number. Headings (6 levels), paragraphs, list
items, quotes, preformatted code, rules and pictures. It uses the standard Helvetica and Courier fonts, so **characters outside
Windows-1252 (other scripts, such as Tamil or Devanagari) are written as `?`** and the message says how many. Embedding a font
program is future work. Text is stored uncompressed; pictures are stored raw (RGB, with a soft mask when they have
transparency).

## Safety

A PDF is untrusted input. The reader caps object nesting (48), page tree depth (40), form nesting (10), decoded bytes per file
(384 MiB total, 96 MiB per stream), operators per page (4 million), page pixels (36 million), image pixels (40 million) and path
size, ignores cycles, never runs anything in the file, and works on a separate thread in the viewer so a hostile file cannot
freeze the screen. Tests mutate small PDFs thousands of times and feed deep nesting, self-including forms and a decompression bomb;
none may panic or hang. `pdf_text` output is marked untrusted for the assistant. The tile reads files only where the person's
file rules allow; saving never overwrites a file (a numbered name is used).

## Tests

`cargo test -p iyakki-pdf` (reader, renderer, writer; PDFs are built inside the tests, nothing is committed),
`cargo test -p iyakki-agent --lib pdftools` (chat tools), `cargo test -p iyakki-shell pdfview` and the browser test
`a_pdf_file_page_opens_in_the_viewer_and_a_page_is_saved_as_pdf`.

## Not done yet

* A launcher tile for the viewer: `PdfApp` is a complete `App`, but adding it to the launcher means a new app id in the policy,
  roles, cloud mode and memory gate (`iyakki-sec/src/policy.rs`, `cloudmode.rs`, `memgate.rs`, `shell.rs`), which other work in
  progress was changing; it was left for one pass.
* A chat tool that opens the viewer on a file.
* Office files (the other half of F-BROW-35) are the media/office package's job (F-MEDIA-01).
* Encryption, embedded font rendering, printing to a printer (see the printing work).
