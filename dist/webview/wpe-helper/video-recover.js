// Player recovery for YouTube (docs/development/BROWSER-VIDEO.md). Injected by helper.c into the top frame at document start.
// YouTube streams the video in short answers and, after about one minute, stops sending media to a page whose "proof of origin" token
// it does not accept; the player then shows "Something went wrong. Refresh or try again later." (found on a Lenovo, 2026-11-16: the
// answers carry a protection status, 2 = still checking, 3 = required, and no media after 60 s of video). The token is made by a
// program of YouTube's inside the page; whether it is accepted changes from one page load to the next. When the player shows that
// error, this loads the page again at the second the video had reached (a new page makes a new token) and then presses the player's play
// button once, as the person had done to start it: the page cannot start a video by itself (YouTube ignores a script's click), so it
// asks the helper for one real click at the button ("playclick X Y", which the helper only passes on for a youtube.com page, not more
// often than every 5 s and 8 times in 10 minutes). At most 6 reloads in 10 minutes for one tab; the person can
// still press Reload. Only on youtube.com pages that have a player.
(function () {
  try {
    if (window.__iyakkiRecover || window.top !== window) return;
    if (!/(^|\.)youtube\.com$/.test(location.hostname)) return;
    window.__iyakkiRecover = 1;
    var MH = null; try { MH = window.webkit.messageHandlers.iyakkiMedia; } catch (e) {}
    var dlog = function (m) { try { if (MH) MH.postMessage('diag recover ' + m); } catch (e) {} };
    var KEY = 'iyakkiVideoRecover', MAX = 6, WINDOW_MS = 10 * 60 * 1000, RKEY = 'iyakkiVideoResume';
    var load = function () { try { return JSON.parse(sessionStorage.getItem(KEY) || '[]'); } catch (e) { return []; } };
    var save = function (a) { try { sessionStorage.setItem(KEY, JSON.stringify(a)); } catch (e) {} };
    var failed = function () {
      var e = document.querySelector('.ytp-error');
      if (!e) return false;
      var cs = window.getComputedStyle(e);
      return cs.display !== 'none' && cs.visibility !== 'hidden' && e.offsetHeight > 0;
    };
    var played = 0;       // the furthest second the video reached on this page load
    var again = function () {
      if (/^\/(watch|live)/.test(location.pathname) === false) return;
      var now = Date.now(), tries = load().filter(function (t) { return now - t < WINDOW_MS; });
      if (tries.length >= MAX) { dlog('giving up after ' + tries.length + ' tries'); return; }
      tries.push(now); save(tries);
      var at = played > 3 ? Math.max(0, Math.floor(played) - 2) : 0;
      dlog('error shown, loading the page again (try ' + tries.length + ') at ' + at + ' s');
      try { sessionStorage.setItem(RKEY, String(now)); } catch (e) {}
      var u = new URL(location.href);
      if (at > 0) u.searchParams.set('t', at + 's');
      location.replace(u.toString());
    };
    document.addEventListener('timeupdate', function (e) {
      var t = e.target;
      if (t && t.tagName === 'VIDEO' && t.currentTime > played && !t.paused) played = t.currentTime;
    }, true);
    // the page made by a reload of ours: press its play button once (a real click through the helper)
    try {
      var mark = parseInt(sessionStorage.getItem(RKEY) || '0', 10);
      dlog('load, reload mark ' + (mark ? (Date.now() - mark) + ' ms old' : 'none'));
      if (mark && Date.now() - mark < 30000) {
        var n = 0, go = setInterval(function () {
          var v = document.querySelector('video');
          if (++n > 40 || (v && !v.paused && v.currentTime > 1)) { clearInterval(go); try { sessionStorage.removeItem(RKEY); } catch (e) {} return; }
          var b = document.querySelector('.ytp-large-play-button');
          if (b && b.offsetHeight > 0 && !failed() && !(v && !v.paused) && n > 3) {
            var r = b.getBoundingClientRect();
            dlog('asking for a click at the play button');
            try { MH.postMessage('playclick ' + Math.round(r.left + r.width / 2) + ' ' + Math.round(r.top + r.height / 2)); } catch (e) {}
            clearInterval(go); try { sessionStorage.removeItem(RKEY); } catch (e) {}
          }
        }, 750);
      }
    } catch (e) {}
    var seen = 0;
    setInterval(function () {
      if (!failed()) { seen = 0; return; }
      if (++seen >= 2) { seen = 0; again(); }    // the message has stood for two checks (about 2 s)
    }, 1000);
  } catch (e) {}
})();
