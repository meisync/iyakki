#!/usr/bin/env python3
"""Asks a real Chrome (headless, with a WebAuthn VIRTUAL authenticator) for a registration and a sign-in on a localhost page and prints
what it got as `key=value` lines (base64url). Used only by crates/iyakki-webcrypto/tests/chrome_passkey.rs, which checks the answers with
the relying-party code of IYAKKI. Exit 3 when there is no Chrome (the test then says it was skipped). Test only, never shipped."""
import base64, http.server, os, sys, tempfile, threading

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cdp  # noqa: E402


def b64u(b):
    return base64.urlsafe_b64encode(bytes(b)).rstrip(b"=").decode()


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"<!doctype html><title>passkey</title>"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


JS_CREATE = """(async()=>{const c=await navigator.credentials.create({publicKey:{challenge:new Uint8Array(32).fill(0x11),rp:{id:'localhost',name:'t'},
user:{id:new Uint8Array([1,2,3,4]),name:'ana',displayName:'Ana'},pubKeyCredParams:[{type:'public-key',alg:-7}],attestation:'none',
authenticatorSelection:{residentKey:'required',userVerification:'preferred'}}});
const u=b=>btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\\+/g,'-').replace(/\\//g,'_').replace(/=+$/,'');
return {id:u(c.rawId),att:u(c.response.attestationObject),cd:u(c.response.clientDataJSON)}})()"""
JS_GET = """(async()=>{const c=await navigator.credentials.get({publicKey:{challenge:new Uint8Array(32).fill(0x22),rpId:'localhost',userVerification:'preferred'}});
const u=b=>btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\\+/g,'-').replace(/\\//g,'_').replace(/=+$/,'');
return {id:u(c.rawId),ad:u(c.response.authenticatorData),sig:u(c.response.signature),cd:u(c.response.clientDataJSON)}})()"""


def main():
    if not cdp.find_chrome():
        print("no chrome", file=sys.stderr)
        return 3
    srv = http.server.HTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    origin = "http://localhost:%d" % srv.server_address[1]
    with tempfile.TemporaryDirectory() as work, cdp.Chrome(work, hard_limit=120) as ch:
        page = ch.new_page()
        page.goto(origin + "/")
        page.call("WebAuthn.enable")
        page.call("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True,
                  "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})
        made = page.evaluate(JS_CREATE)
        got = page.evaluate(JS_GET)
    print("origin=" + origin)
    for k, v in made.items():
        print("create_%s=%s" % (k, v))
    for k, v in got.items():
        print("get_%s=%s" % (k, v))
    return 0


if __name__ == "__main__":
    sys.exit(main())
