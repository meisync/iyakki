//! `TextEncoder`, `TextDecoder`, `atob`, `btoa`.

use std::any::Any;

use crate::engine::WebModule;

pub struct EncodingModule;

impl WebModule for EncodingModule {
    fn name(&self) -> &'static str {
        "encoding"
    }

    fn prelude(&self) -> &'static str {
        r#"
(function (g) {
  'use strict';
  class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(s = '') {
      s = String(s);
      const out = [];
      for (let i = 0; i < s.length; i++) {
        let c = s.codePointAt(i);
        if (c > 0xffff) i++;
        if (c >= 0xd800 && c <= 0xdfff) c = 0xfffd;
        if (c < 0x80) out.push(c);
        else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
        else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
        else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      }
      return Uint8Array.from(out);
    }
  }
  class TextDecoder {
    constructor(label = 'utf-8') { this.encoding = String(label).toLowerCase(); }
    decode(buf) {
      if (buf === undefined) return '';
      const b = buf instanceof ArrayBuffer ? new Uint8Array(buf) : new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
      if (this.encoding === 'latin1' || this.encoding === 'iso-8859-1' || this.encoding === 'ascii') return String.fromCharCode(...b);
      let s = '';
      for (let i = 0; i < b.length;) {
        const c = b[i];
        let cp = 0xfffd, n = 1;
        if (c < 0x80) cp = c;
        else if (c >= 0xc2 && c < 0xe0 && (b[i + 1] & 0xc0) === 0x80) { cp = ((c & 31) << 6) | (b[i + 1] & 63); n = 2; }
        else if (c >= 0xe0 && c < 0xf0 && (b[i + 1] & 0xc0) === 0x80 && (b[i + 2] & 0xc0) === 0x80) { cp = ((c & 15) << 12) | ((b[i + 1] & 63) << 6) | (b[i + 2] & 63); n = 3; if (cp < 0x800 || (cp >= 0xd800 && cp <= 0xdfff)) cp = 0xfffd; }
        else if (c >= 0xf0 && c < 0xf5 && (b[i + 1] & 0xc0) === 0x80 && (b[i + 2] & 0xc0) === 0x80 && (b[i + 3] & 0xc0) === 0x80) { cp = ((c & 7) << 18) | ((b[i + 1] & 63) << 12) | ((b[i + 2] & 63) << 6) | (b[i + 3] & 63); n = 4; if (cp < 0x10000 || cp > 0x10ffff) cp = 0xfffd; }
        s += String.fromCodePoint(cp);
        i += n;
      }
      return s;
    }
  }
  const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  g.btoa = (s) => {
    s = String(s);
    let out = '';
    for (let i = 0; i < s.length; i += 3) {
      const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) throw new Error('InvalidCharacterError: the string contains characters outside Latin-1');
      const n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += A[(n >> 18) & 63] + A[(n >> 12) & 63] + (i + 1 < s.length ? A[(n >> 6) & 63] : '=') + (i + 2 < s.length ? A[n & 63] : '=');
    }
    return out;
  };
  g.atob = (s) => {
    s = String(s).replace(/[\t\n\f\r ]+/g, '').replace(/=+$/, '');
    if (s.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(s)) throw new Error('InvalidCharacterError: the string is not correctly encoded');
    let out = '', bits = 0, acc = 0;
    for (const ch of s) {
      acc = (acc << 6) | A.indexOf(ch); bits += 6;
      if (bits >= 8) { bits -= 8; out += String.fromCharCode((acc >> bits) & 255); }
    }
    return out;
  };
  g.TextEncoder = TextEncoder;
  g.TextDecoder = TextDecoder;
})(globalThis);
"#
    }

    fn call(&mut self, op: &str, _args: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err(format!("encoding has no operation {op}"))
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JsEngine, Limits};

    fn run(src: &str) -> String {
        let mut e = JsEngine::new(Limits::default(), vec![Box::new(EncodingModule)]).unwrap();
        e.eval("t", src).unwrap()
    }

    #[test]
    fn text_encoder_and_decoder_round_trip_utf8() {
        assert_eq!(run("Array.from(new TextEncoder().encode('aé€😀')).join(',')"), "97,195,169,226,130,172,240,159,152,128");
        assert_eq!(run("new TextDecoder().decode(new TextEncoder().encode('héllo — 日本語 😀'))"), "héllo — 日本語 😀");
        assert_eq!(run("new TextDecoder().decode(new Uint8Array([0xff, 0x41, 0xc3]))"), "\u{fffd}A\u{fffd}", "bad bytes become replacement characters");
        assert_eq!(run("new TextDecoder('latin1').decode(new Uint8Array([0xe9]))"), "é");
        assert_eq!(run("new TextDecoder().decode()"), "");
    }

    #[test]
    fn base64() {
        assert_eq!(run("btoa('hello, world')"), "aGVsbG8sIHdvcmxk");
        assert_eq!(run("btoa('a')+btoa('ab')+btoa('')"), "YQ==YWI=");
        assert_eq!(run("atob('aGVsbG8sIHdvcmxk')"), "hello, world");
        assert_eq!(run("atob(' YW Jj\\n')"), "abc", "white space is ignored");
        assert_eq!(run("try { atob('a*b'); 'no' } catch (e) { 'threw' }"), "threw");
        assert_eq!(run("try { btoa('日本'); 'no' } catch (e) { 'threw' }"), "threw");
    }
}
