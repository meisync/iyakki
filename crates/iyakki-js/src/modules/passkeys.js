(function (g) {
  'use strict';
  if (!g.isSecureContext || !g.navigator) return;
  const call = (op, a) => JSON.parse(__host('passkeys', op, JSON.stringify(a)));
  const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  const bytesOf = (v, what) => {
    if (v instanceof ArrayBuffer) return new Uint8Array(v);
    if (ArrayBuffer.isView(v)) return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
    throw new TypeError(what + ' must be an ArrayBuffer or a typed array');
  };
  const enc = (b) => {
    let o = '';
    for (let i = 0; i < b.length; i += 3) {
      const n = (b[i] << 16) | ((b[i + 1] || 0) << 8) | (b[i + 2] || 0);
      o += A[(n >> 18) & 63] + A[(n >> 12) & 63] + (i + 1 < b.length ? A[(n >> 6) & 63] : '') + (i + 2 < b.length ? A[n & 63] : '');
    }
    return o;
  };
  const dec = (s) => {
    const out = [];
    let acc = 0, bits = 0;
    for (const ch of String(s)) {
      const v = A.indexOf(ch);
      if (v < 0) continue;
      acc = (acc << 6) | v; bits += 6;
      if (bits >= 8) { bits -= 8; out.push((acc >> bits) & 255); acc &= (1 << bits) - 1; }
    }
    return new Uint8Array(out).buffer;
  };
  const fail = (m) => {
    m = String(m);
    const k = m.indexOf(':');
    const names = ['NotAllowedError', 'SecurityError', 'InvalidStateError', 'NotSupportedError', 'ConstraintError', 'TypeError'];
    if (k > 0 && names.indexOf(m.slice(0, k)) >= 0) return m.slice(0, k) === 'TypeError' ? new TypeError(m.slice(k + 1).trim()) : new DOMException(m.slice(k + 1).trim(), m.slice(0, k));
    return new DOMException(m, 'NotAllowedError');
  };
  const ids = (list, what) => {
    if (list === undefined || list === null) return [];
    if (!Array.isArray(list) || list.length > 64) throw new TypeError(what + ' is a list of at most 64 credentials');
    return list.filter((c) => c && c.type === 'public-key').map((c) => enc(bytesOf(c.id, 'a credential id')));
  };
  class AuthenticatorResponse { constructor(clientDataJSON) { this.clientDataJSON = clientDataJSON; } }
  class AuthenticatorAttestationResponse extends AuthenticatorResponse {
    constructor(c, a, pk, transports) { super(c); this.attestationObject = a; this._pk = pk; this._t = Array.isArray(transports) ? transports.map(String) : ['internal']; }
    getTransports() { return this._t.slice(); }
    getPublicKeyAlgorithm() { return -7; }
    getAuthenticatorData() { return null; }
  }
  class AuthenticatorAssertionResponse extends AuthenticatorResponse {
    constructor(c, a, s, u) { super(c); this.authenticatorData = a; this.signature = s; this.userHandle = u; }
  }
  class PublicKeyCredential {
    constructor(id, raw, response, attachment) { this.id = id; this.rawId = raw; this.type = 'public-key'; this.response = response; this.authenticatorAttachment = attachment === 'cross-platform' ? 'cross-platform' : 'platform'; }
    getClientExtensionResults() { return {}; }
    toJSON() { return { id: this.id, rawId: this.id, type: this.type }; }
    static isUserVerifyingPlatformAuthenticatorAvailable() { return Promise.resolve(false); }
    static isConditionalMediationAvailable() { return Promise.resolve(false); }
    static getClientCapabilities() { return Promise.resolve({ userVerifyingPlatformAuthenticator: false, conditionalGet: false, conditionalCreate: false }); }
  }
  const pre = (o) => {
    if (!o || typeof o !== 'object') throw new TypeError('the options of credentials are an object');
    if (o.signal && o.signal.aborted) throw o.signal.reason || new DOMException('The operation was aborted.', 'AbortError');
    if (o.mediation === 'conditional') throw new DOMException('Conditional sign-in is not supported here; the page can ask for the passkey with a click.', 'NotSupportedError');
    if (o.publicKey === undefined) throw new DOMException('Only public key credentials (passkeys) are supported.', 'NotSupportedError');
    return o.publicKey;
  };
  const credentials = {
    create(o) {
      return new Promise((resolve, reject) => {
        try {
          const p = pre(o);
          if (!p.rp || !p.user) throw new TypeError('a publicKey needs rp and user');
          const params = Array.isArray(p.pubKeyCredParams) ? p.pubKeyCredParams : [];
          if (params.length > 0 && !params.some((x) => x && x.type === 'public-key' && x.alg === -7)) throw new DOMException('Only ES256 (-7) passkeys can be made here.', 'NotSupportedError');
          const sel = p.authenticatorSelection || {};
          const r = call('create', {
            rpId: p.rp.id === undefined ? '' : String(p.rp.id), rpName: String(p.rp.name || ''), challenge: enc(bytesOf(p.challenge, 'the challenge')),
            userHandle: enc(bytesOf(p.user.id, 'the user id')), userName: String(p.user.name || p.user.displayName || ''),
            exclude: ids(p.excludeCredentials, 'excludeCredentials'),
            attachment: sel.authenticatorAttachment === 'cross-platform' ? 'cross-platform' : '', uv: String(sel.userVerification || p.userVerification || ''),
            resident: sel.residentKey === 'required' || sel.requireResidentKey === true,
          });
          if (r.error) throw fail(r.error);
          resolve(new PublicKeyCredential(r.credentialId, dec(r.credentialId), new AuthenticatorAttestationResponse(dec(r.clientDataJSON), dec(r.attestationObject), null, r.transports), r.attachment));
        } catch (e) { reject(e && e.message !== undefined && !(e instanceof DOMException) && !(e instanceof TypeError) ? fail(e.message) : e); }
      });
    },
    get(o) {
      return new Promise((resolve, reject) => {
        try {
          const p = pre(o);
          const r = call('get', { rpId: p.rpId === undefined ? '' : String(p.rpId), challenge: enc(bytesOf(p.challenge, 'the challenge')), allow: ids(p.allowCredentials, 'allowCredentials'), uv: String(p.userVerification || '') });
          if (r.error) throw fail(r.error);
          resolve(new PublicKeyCredential(r.credentialId, dec(r.credentialId), new AuthenticatorAssertionResponse(dec(r.clientDataJSON), dec(r.authenticatorData), dec(r.signature), r.userHandle ? dec(r.userHandle) : null), r.attachment));
        } catch (e) { reject(e && e.message !== undefined && !(e instanceof DOMException) && !(e instanceof TypeError) ? fail(e.message) : e); }
      });
    },
    store(c) { return Promise.resolve(c); },
    preventSilentAccess() { return Promise.resolve(); },
  };
  Object.defineProperty(g.navigator, 'credentials', { value: credentials, configurable: true, enumerable: true });
  g.PublicKeyCredential = PublicKeyCredential; g.AuthenticatorResponse = AuthenticatorResponse;
  g.AuthenticatorAttestationResponse = AuthenticatorAttestationResponse; g.AuthenticatorAssertionResponse = AuthenticatorAssertionResponse;
})(globalThis);
