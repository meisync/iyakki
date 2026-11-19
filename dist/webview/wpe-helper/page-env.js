// Page environment (docs/development/BROWSER-VIDEO.md). Injected by helper.c into every frame at document start, after
// `window.__iyakkiScreen = [width, height, scale]` (the size of the page area the shell draws, in CSS pixels).
// The offscreen engine has no display of its own: `screen` answers 0 x 0 and the window position is 0, which no real browser does. Sites that
// check the environment before they hand out streams (YouTube's attestation of the player runs a program that reads the screen, the window
// and the plugins) judged such a page a robot in about one start in four and then stopped sending video after one minute ("Something went
// wrong"). This gives the page the values a browser on that screen has. It changes nothing when the engine already reports a screen.
(function () {
  try {
    var s = window.__iyakkiScreen;
    if (!s || window.__iyakkiEnv) return;
    window.__iyakkiEnv = 1;
    var S = window.screen;
    if (!S || S.width > 0) return;
    var w = Math.max(1, s[0] | 0), h = Math.max(1, s[1] | 0);
    var def = function (o, k, g) { try { Object.defineProperty(o, k, { get: g, configurable: true, enumerable: true }); } catch (e) {} };
    // never smaller than the page itself (the page area can grow after the first picture)
    var cw = function () { return Math.max(w, window.innerWidth | 0); }, ch = function () { return Math.max(h, window.innerHeight | 0); };
    def(S, 'width', cw); def(S, 'height', ch); def(S, 'availWidth', cw); def(S, 'availHeight', ch);
    def(S, 'availLeft', function () { return 0; }); def(S, 'availTop', function () { return 0; });
    if (!S.colorDepth) { def(S, 'colorDepth', function () { return 24; }); def(S, 'pixelDepth', function () { return 24; }); }
    var win = window;
    var live = function (k, inner) { try { if (!win[k]) Object.defineProperty(win, k, { get: function () { return win[inner]; }, configurable: true, enumerable: true }); } catch (e) {} };
    live('outerWidth', 'innerWidth'); live('outerHeight', 'innerHeight');
  } catch (e) {}
})();
