// Reads filled-in PDF forms with Apple PDFKit (a reader that is not ours) and prints every field with its value.
// Run on a Mac:
//   IYAKKI_FORM_OUT=/some/folder cargo test -p iyakki-pdf --lib write_filled_samples -- --ignored
//   swiftc -o check-forms check-forms.swift && ./check-forms /some/folder/classic-filled.pdf /some/folder/xrefstream-filled.pdf
// Expected: classic-filled.pdf shows name = Grace Hopper, agree = Yes (state 1), colour = green; xrefstream-filled.pdf shows
// name = Ada Lovelace. Written for F-BROX-08 (docs/PDF.md).
import PDFKit
for n in CommandLine.arguments.dropFirst() {
    guard let d = PDFDocument(url: URL(fileURLWithPath: n)) else { print(n, "NOT OPENED"); continue }
    print(n, "pages", d.pageCount)
    for a in d.page(at: 0)!.annotations {
        print("  ", a.fieldName ?? "?", "=", a.widgetStringValue ?? "nil", "state", a.buttonWidgetState.rawValue)
    }
}
