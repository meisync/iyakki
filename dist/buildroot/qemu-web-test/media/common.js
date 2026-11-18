// Shared by the media test pages (docs/development/BROWSER-MEDIA-DEVICES.md, "Test pages"). Each page runs one check and reports it three ways:
// the page title ("MEDIA-TEST <name> PASS|FAIL <detail>", which the shell and the helper log see), the #result element, and a request to
// /report?... of qemu-web-test/server.py. `?expect=denied` turns the page around: PASS means the browser refused with NotAllowedError.
(function () {
  var q = new URLSearchParams(location.search);
  window.mediaTest = {
    expectDenied: q.get('expect') === 'denied',
    secure: window.isSecureContext,
    report: function (name, ok, detail) {
      var line = 'MEDIA-TEST ' + name + ' ' + (ok ? 'PASS' : 'FAIL') + ' ' + (detail || '');
      document.title = line;
      var el = document.getElementById('result');
      if (el) { el.textContent = line; el.className = ok ? 'pass' : 'fail'; }
      try { fetch('/report?' + encodeURIComponent(line)); } catch (e) {}
    },
    // run `fn` (returns a promise of a detail string), within `ms`, honouring ?expect=denied
    run: function (name, ms, fn) {
      var self = this, done = false;
      var finish = function (ok, d) { if (!done) { done = true; self.report(name, ok, d); } };
      setTimeout(function () { finish(false, 'timeout after ' + ms + ' ms'); }, ms);
      if (!navigator.mediaDevices) return finish(false, 'navigator.mediaDevices is undefined (secure context: ' + self.secure + ')');
      Promise.resolve().then(fn).then(function (d) {
        finish(!self.expectDenied, self.expectDenied ? 'expected a refusal but it worked: ' + d : d);
      }, function (e) {
        var denied = e && e.name === 'NotAllowedError';
        finish(self.expectDenied && denied, (e && e.name) + ': ' + (e && e.message));
      });
    }
  };
})();
