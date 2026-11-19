// Makes the encrypted test files next to it with macOS's own PDF writer (CoreGraphics), so they are not made by this repository's code:
//   swiftc -o gen make-encrypted-fixtures.swift && ./gen
// aes-128.pdf is what CoreGraphics writes for a 128-bit key (V4, AESV2); rc4-40.pdf is V1/R2 with RC4. User password open-sesame, owner password owner-key.
import Foundation
import CoreGraphics
import CoreText
func make(_ path: String, user: String?, owner: String, bits: Int) {
    var info: [CFString: Any] = [kCGPDFContextOwnerPassword: owner, kCGPDFContextEncryptionKeyLength: bits]
    if let u = user { info[kCGPDFContextUserPassword] = u }
    var box = CGRect(x: 0, y: 0, width: 300, height: 200)
    let url = URL(fileURLWithPath: path) as CFURL
    guard let ctx = CGContext(url, mediaBox: &box, info as CFDictionary) else { print("no context"); return }
    ctx.beginPDFPage(nil)
    let attr = NSAttributedString(string: "Secret words of the encrypted page", attributes: [kCTFontAttributeName as NSAttributedString.Key: CTFontCreateWithName("Helvetica" as CFString, 18, nil)])
    let line = CTLineCreateWithAttributedString(attr)
    ctx.textPosition = CGPoint(x: 20, y: 100)
    CTLineDraw(line, ctx)
    ctx.endPDFPage()
    ctx.closePDF()
}
make("aes-128.pdf", user: "open-sesame", owner: "owner-key", bits: 128)
make("rc4-40.pdf", user: "open-sesame", owner: "owner-key", bits: 40)
make("owner-only-aes-128.pdf", user: nil, owner: "owner-key", bits: 128)
