// Developer-tools probe (docs/WEBVIEW-PROTOCOL.md, DevEvent). Injected by helper.c into every frame at document start.
// It watches the page the way a browser's developer tools do and posts one short text line per event to the "iyakkiDev" message
// handler: failed requests (fetch, XHR, resources that did not load), slow resources, console errors and warnings, uncaught errors
// and rejected promises, and the page's load timings. Fields are separated by U+001F, the first is a kind letter:
//   F method status ms url error      a failed request (status 0 = no answer: network error, blocked, aborted)
//   R initiator bytes ms url          a slow resource (300 ms or more)
//   C level line col source text      a console error/warning or an uncaught error (level: error, warn)
//   P response_ms dcl_ms load_ms url  page timings in ms after navigation start (top frame, once per load)
//   H                                 hello: the probe is running (top frame, once per document, at document start)
// No request or response bodies, no headers and no cookies are read; texts are cut short here, and the shell scrubs addresses
// (query, fragment, tokens) before it stores anything. The helper caps the lines per page load.
(function () {
  try {
    if (window.__iyakkiDevProbe) return;
    window.__iyakkiDevProbe = 1;
    var SEP = '\u001f';
    var cut = function (s, n) { s = String(s == null ? '' : s); return s.length > n ? s.slice(0, n) : s; };
    var post = function (kind, a, b, c, d, e) {
      try {
        window.webkit.messageHandlers.iyakkiDev.postMessage([kind, cut(a, 40), cut(b, 12), cut(c, 12), cut(d, 300), cut(e, 300)].join(SEP).replace(/[\r\n]+/g, ' '));
      } catch (er) {}
    };
    var abs = function (u) { try { return String(new URL(String(u), document.baseURI)); } catch (er) { return String(u); } };
    var ms = function () { return Math.round(performance.now()); };

    if (window === window.top) post('H', '', '', '', '', '');

    // fetch: an answer with status 400 or more, or no answer at all
    if (window.fetch) {
      var of = window.fetch;
      window.fetch = function (input, init) {
        var t0 = ms(), url = abs(input && input.url ? input.url : input), m = (init && init.method) || (input && input.method) || 'GET';
        var p = of.apply(this, arguments);
        try {
          p.then(function (r) { if (r && r.status >= 400) post('F', m, r.status, ms() - t0, url, r.statusText); },
                 function (er) { post('F', m, 0, ms() - t0, url, er && er.name === 'AbortError' ? 'aborted' : 'network error'); });
        } catch (er) {}
        return p;
      };
    }

    // XMLHttpRequest
    var X = window.XMLHttpRequest && XMLHttpRequest.prototype;
    if (X && X.open && X.send) {
      var oo = X.open, os = X.send;
      X.open = function (m, u) { try { this.__dm = m; this.__du = abs(u); } catch (er) {} return oo.apply(this, arguments); };
      X.send = function () {
        var x = this, t0 = ms();
        try {
          x.addEventListener('loadend', function () {
            if (x.status === 0 || x.status >= 400) post('F', x.__dm || 'GET', x.status, ms() - t0, x.__du || '', x.status ? x.statusText : 'network error');
          });
        } catch (er) {}
        return os.apply(this, arguments);
      };
    }

    // pictures, scripts and style sheets that did not load (resource errors do not bubble: use the capture phase)
    window.addEventListener('error', function (e) {
      var t = e.target;
      if (t && t !== window && t.tagName) {
        var u = t.currentSrc || t.src || t.href || '';
        if (u) post('F', 'GET', 0, 0, abs(u), String(t.tagName).toLowerCase() + ' failed to load');
      } else {
        post('C', 'error', e.lineno || 0, e.colno || 0, e.filename || '', e.message || 'script error');
      }
    }, true);
    window.addEventListener('unhandledrejection', function (e) {
      var r = e.reason;
      post('C', 'error', 0, 0, '', 'Unhandled promise rejection: ' + (r && r.message ? r.message : r));
    });

    // console.error and console.warn (the page still gets the original behaviour)
    var wrap = function (name, level) {
      var c = window.console;
      if (!c || !c[name]) return;
      var o = c[name];
      c[name] = function () {
        try {
          var parts = [];
          for (var i = 0; i < arguments.length && i < 6; i++) {
            var a = arguments[i];
            parts.push(a && a.stack ? String(a.message || a) : (typeof a === 'object' ? '[object]' : String(a)));
          }
          post('C', level, 0, 0, location.href, parts.join(' '));
        } catch (er) {}
        return o.apply(this, arguments);
      };
    };
    wrap('error', 'error');
    wrap('warn', 'warn');

    // slow resources and answers with an error status, from the engine's own resource timing
    if (window.PerformanceObserver) {
      try {
        new PerformanceObserver(function (list) {
          list.getEntries().forEach(function (e) {
            if (e.responseStatus >= 400) post('F', 'GET', e.responseStatus, Math.round(e.duration), e.name, '');
            else if (e.duration >= 300) post('R', e.initiatorType, e.transferSize || 0, Math.round(e.duration), e.name, '');
          });
        }).observe({ type: 'resource', buffered: true });
      } catch (er) {}
    }

    // page timings, once, a moment after the load event
    if (window === window.top) {
      window.addEventListener('load', function () {
        setTimeout(function () {
          try {
            var n = performance.getEntriesByType('navigation')[0];
            if (n) post('P', Math.round(n.responseStart), Math.round(n.domContentLoadedEventEnd), Math.round(n.loadEventEnd), location.href, '');
          } catch (er) {}
        }, 0);
      });
    }
  } catch (e) {}
})();
