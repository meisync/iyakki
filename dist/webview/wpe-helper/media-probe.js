// Media probe (docs/WEBVIEW-PROTOCOL.md, MediaUnsupported). Injected by helper.c into every frame at document start.
// When a page's video or sound fails because the engine has no H.264 / AAC decoder, tell the shell once, so it can show one calm
// message instead of a black player. It reports only when the engine's own canPlayType agrees that the codec is missing, so a
// broken file or a network error never produces the message. Sent to the "iyakkiMedia" message handler: "video", "audio" or "both".
(function () {
  try {
    if (window.__iyakkiMediaProbe) return;
    window.__iyakkiMediaProbe = 1;
    var sent = false;
    var post = function (m) { try { window.webkit.messageHandlers.iyakkiMedia.postMessage(m); } catch (e) {} };
    var can = function (t) {
      try { return document.createElement('video').canPlayType(t) !== ''; } catch (e) { return true; }
    };
    var need = function () {
      var v = !can('video/mp4; codecs="avc1.42E01E"'), a = !can('audio/mp4; codecs="mp4a.40.2"');
      return v && a ? 'both' : v ? 'video' : a ? 'audio' : '';
    };
    var report = function () {
      if (sent) return;
      var n = need();
      if (!n) return;
      sent = true;
      post(n);
    };

    // Diagnosis lines ("diag ..."): what a media element and Media Source did, for the helper log while the developer switch
    // /tmp/iyakki-dev/gst-debug exists (a player that shows "Something went wrong" says why here). At most 300 lines per page load.
    if (window.__iyakkiDiag) {
    var dn = 0;
    var dlog = function (s) { if (dn++ < 800) post('diag ' + String(s).slice(0, 380)); };
    var ranges = function (b) { var o = []; try { for (var i = 0; i < b.length && i < 4; i++) o.push(b.start(i).toFixed(1) + '-' + b.end(i).toFixed(1)); } catch (e) {} return o.join(','); };
    var vinfo = function (v) {
      var q = {}; try { q = v.getVideoPlaybackQuality ? v.getVideoPlaybackQuality() : {}; } catch (e) {}
      var er = v.error ? (v.error.code + ' ' + (v.error.message || '')) : '-';
      return 't=' + (v.currentTime || 0).toFixed(1) + ' rs=' + v.readyState + ' ns=' + v.networkState + ' buf=[' + ranges(v.buffered) + '] size=' + v.videoWidth + 'x' + v.videoHeight +
        ' drop=' + (q.droppedVideoFrames | 0) + '/' + (q.totalVideoFrames | 0) + ' err=' + er;
    };
    ['error', 'stalled', 'waiting', 'emptied', 'abort', 'ended', 'seeking', 'ratechange'].forEach(function (n) {
      document.addEventListener(n, function (e) { var t = e.target; if (t && /^(VIDEO|AUDIO)$/.test(t.tagName || '')) dlog(n + ' ' + t.tagName.toLowerCase() + ' ' + vinfo(t)); }, true);
    });
    setInterval(function () {
      var vs = document.getElementsByTagName('video');
      for (var i = 0; i < vs.length && i < 3; i++) if (!vs[i].paused) dlog('tick ' + vinfo(vs[i]) + ' net=' + fstart + '/' + fok + '/' + ffail + ' pending=' + fpend);
    }, 5000);
    window.addEventListener('error', function (e) { dlog('window error ' + (e && e.message)); });
    window.addEventListener('unhandledrejection', function (e) { dlog('rejection ' + (e && e.reason && (e.reason.name + ': ' + e.reason.message))); });
    setTimeout(function () { try {
      var gl = document.createElement('canvas').getContext('webgl'), ex = gl && gl.getExtension('WEBGL_debug_renderer_info');
      dlog('env ua=' + navigator.userAgent + ' | webdriver=' + navigator.webdriver + ' plugins=' + (navigator.plugins && navigator.plugins.length) + ' mimes=' + (navigator.mimeTypes && navigator.mimeTypes.length) +
        ' lang=' + navigator.languages + ' cores=' + navigator.hardwareConcurrency + ' mem=' + navigator.deviceMemory + ' screen=' + screen.width + 'x' + screen.height + '@' + devicePixelRatio + ' outer=' + window.outerWidth + 'x' + window.outerHeight + ' inner=' + window.innerWidth + 'x' + window.innerHeight + ' avail=' + screen.availWidth + ' cd=' + screen.colorDepth +
        ' vendor=' + navigator.vendor + ' platform=' + navigator.platform + ' gl=' + (gl ? (ex ? gl.getParameter(ex.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER)) : 'none') +
        ' uad=' + (typeof navigator.userAgentData) + ' chrome=' + (typeof window.chrome) + ' notif=' + (window.Notification && Notification.permission) + ' tz=' + Intl.DateTimeFormat().resolvedOptions().timeZone);
    } catch (e) { dlog('env failed ' + e); } }, 4000);
    var fpend = 0, fstart = 0, fok = 0, ffail = 0;
    var qinfo = function (u) {
      var o = []; try { var q = new URL(u, location.href); ['itag', 'range', 'rn', 'sabr', 'c', 'mime', 'ump', 'clen'].forEach(function (k) { var v = q.searchParams.get(k); if (v !== null) o.push(k + '=' + String(v).slice(0, 24)); }); o.unshift(q.hostname.replace(/^[^.]+/, 'h')); } catch (e) {}
      return o.join(' ');
    };
    // YouTube's server-driven streaming answers (UMP): read a copy of each answer and log the kinds of parts it carried
    var ump = function (id, x, t0) {
      try {
        var rd = x.clone().body.getReader(), buf = new Uint8Array(0), cnt = {}, total = 0, extra = [];
        var vint = function (b, o) { // UMP variable-length integer; returns [value, length] or null
          if (o >= b.length) return null; var f = b[o], n = f < 128 ? 1 : f < 192 ? 2 : f < 224 ? 3 : f < 240 ? 4 : 5;
          if (o + n > b.length) return null; var v;
          if (n === 1) v = f; else if (n === 2) v = (f & 63) + 64 * b[o + 1]; else if (n === 3) v = (f & 31) + 32 * (b[o + 1] + 256 * b[o + 2]);
          else if (n === 4) v = (f & 15) + 16 * (b[o + 1] + 256 * (b[o + 2] + 256 * b[o + 3])); else v = b[o + 1] + 256 * (b[o + 2] + 256 * (b[o + 3] + 256 * b[o + 4]));
          return [v, n];
        };
        var parse = function () {
          var o = 0;
          for (;;) {
            var a = vint(buf, o); if (!a) break; var b = vint(buf, o + a[1]); if (!b) break;
            var st = o + a[1] + b[1]; if (st + b[0] > buf.length) { if (a[0] === 21) { /* media bytes: count without waiting */ } break; }
            cnt[a[0]] = (cnt[a[0]] || 0) + 1;
            if (a[0] === 58 || a[0] === 44 || a[0] === 43 || a[0] === 46 || a[0] === 35) extra.push(a[0] + ':' + Array.prototype.slice.call(buf.subarray(st, Math.min(st + b[0], st + 24))).join('.'));
            o = st + b[0];
          }
          buf = buf.subarray(o);
        };
        (function pump() {
          rd.read().then(function (d) {
            if (d.done) { dlog('fetch#' + id + ' body ' + total + ' bytes in ' + (Date.now() - t0) + 'ms parts ' + JSON.stringify(cnt) + (extra.length ? ' special ' + extra.slice(0, 6).join(' ') : '')); return; }
            total += d.value.length; var nb = new Uint8Array(buf.length + d.value.length); nb.set(buf, 0); nb.set(d.value, buf.length); buf = nb; parse(); pump();
          }, function (er) { dlog('fetch#' + id + ' body read FAILED after ' + total + ' bytes ' + (er && er.name) + ' ' + (er && er.message)); });
        })();
      } catch (e) { dlog('ump sniff failed ' + e); }
    };
    if (window.fetch) {
      var of = window.fetch;
      window.fetch = function (a, init) {
        var u = ''; try { u = typeof a === 'string' ? a : a.url; } catch (e) {}
        var r = of.apply(this, arguments);
        if (/googlevideo/.test(u)) {
          var t0 = Date.now(), id = ++fstart, m = (init && init.method) || (a && a.method) || 'GET'; fpend++;
          var bl = -1; try { var bd = init && init.body; bl = bd ? (bd.byteLength !== undefined ? bd.byteLength : String(bd).length) : 0; } catch (e) {}
          dlog('fetch#' + id + ' start ' + m + ' ' + qinfo(u) + ' body=' + bl);
          if (m === 'POST' && a && typeof a !== 'string' && a.clone) { try { a.clone().arrayBuffer().then(function (b) { dlog('fetch#' + id + ' request body ' + b.byteLength + ' bytes'); }); } catch (e) {} }
          r.then(function (x) { fpend--; fok++; dlog('fetch#' + id + ' headers ' + x.status + ' after ' + (Date.now() - t0) + 'ms'); if (m === 'POST') ump(id, x, t0); },
                 function (er) { fpend--; ffail++; dlog('fetch#' + id + ' FAILED after ' + (Date.now() - t0) + 'ms ' + (er && er.name) + ' ' + (er && er.message)); });
        } else {
          r.then(function (x) { if (!/(ggpht|ytimg|fonts|doubleclick)/.test(u)) dlog('fetch ' + x.status + ' ' + String(u).replace(/[?].*$/, '').replace(/^https?:[/][/]/, '').replace(/[.]/g, '_').slice(0, 100)); }, function (er) { dlog('fetch failed ' + (u.replace(/[?].*$/, '').replace(/^https?:[/][/]/, '').replace(/[.]/g, '_').slice(0, 100)) + ' ' + (er && er.name)); });
        }
        return r;
      };
    }
    if (window.XMLHttpRequest) {
      var ox = XMLHttpRequest.prototype.open, os = XMLHttpRequest.prototype.send;
      XMLHttpRequest.prototype.open = function (m, u) { this.__iu = String(u); this.__im = m; return ox.apply(this, arguments); };
      XMLHttpRequest.prototype.send = function () {
        var x = this, u = x.__iu || '';
        if (/googlevideo/.test(u)) {
          var t0 = Date.now(), id = ++fstart; fpend++;
          dlog('xhr#' + id + ' start ' + x.__im + ' ' + qinfo(u));
          x.addEventListener('loadend', function () { fpend--; dlog('xhr#' + id + ' end ' + x.status + ' after ' + (Date.now() - t0) + 'ms'); });
        }
        else if (!/(ggpht|ytimg|fonts|doubleclick)/.test(u)) {
          var xn = String(u).replace(/[?].*$/, '').replace(/^https?:[/][/]/, '').replace(/[.]/g, '_').slice(0, 100);
          x.addEventListener('loadend', function () { dlog('xhr ' + x.status + ' ' + xn); });
        }
        return os.apply(this, arguments);
      };
    }
    if (navigator.sendBeacon) {
      var ob = navigator.sendBeacon;
      navigator.sendBeacon = function (u) { dlog('beacon ' + String(u).replace(/[?].*$/, '').replace(/^https?:[/][/]/, '').replace(/[.]/g, '_').slice(0, 100)); return ob.apply(this, arguments); };
    }
    if (window.Worker) {
      var OW = window.Worker;
      window.Worker = function (u, o) { dlog("worker " + String(u).slice(0, 60)); return new OW(u, o); };
      window.Worker.prototype = OW.prototype;
    }
    if (window.MediaSource) {
      var oe = MediaSource.prototype.endOfStream;
      if (oe) MediaSource.prototype.endOfStream = function (er) { dlog('endOfStream ' + (er || '-') + ' state=' + this.readyState); return oe.apply(this, arguments); };
      var oa2 = MediaSource.prototype.addSourceBuffer;
      MediaSource.prototype.addSourceBuffer = function (t) {
        var sb = oa2.apply(this, arguments);
        dlog('addSourceBuffer ' + t);
        ['error', 'abort'].forEach(function (n) { sb.addEventListener(n, function () { dlog('sourcebuffer ' + n + ' ' + t); }); });
        var oab = sb.appendBuffer;
        sb.appendBuffer = function () { try { return oab.apply(this, arguments); } catch (er) { dlog('appendBuffer threw ' + er.name + ' ' + er.message + ' ' + t); throw er; } };
        var orm = sb.remove;
        sb.remove = function (a, b) { dlog('remove ' + String(a).slice(0, 8) + '-' + String(b).slice(0, 8) + ' ' + t.slice(0, 12)); return orm.apply(this, arguments); };
        return sb;
      };
      ['sourceended', 'sourceclose'].forEach(function (n) { try { document.addEventListener(n, function () { dlog(n); }, true); } catch (e) {} });
    }
    }
    var isMedia = function (t) { return t && /^(VIDEO|AUDIO)$/.test(t.tagName || ''); };
    // an error on a media element (SRC_NOT_SUPPORTED = 4, DECODE = 3) or on its last <source> (NETWORK_NO_SOURCE = 3)
    document.addEventListener('error', function (e) {
      var t = e.target;
      if (isMedia(t)) {
        var c = t.error && t.error.code;
        if (c === 4 || c === 3) report();
      } else if (t && t.tagName === 'SOURCE' && isMedia(t.parentNode) && t.parentNode.networkState === 3) {
        report();
      }
    }, true);
    // a video that has metadata but never gets a picture size: the sound may play, the picture is black
    document.addEventListener('loadedmetadata', function (e) {
      var t = e.target;
      if (t && t.tagName === 'VIDEO') {
        setTimeout(function () { if (t.readyState >= 1 && t.videoWidth === 0 && t.duration > 0) report(); }, 1500);
      }
    }, true);
    // play() rejected because the source is not supported
    var P = window.HTMLMediaElement && HTMLMediaElement.prototype;
    if (P && P.play) {
      var op = P.play;
      P.play = function () {
        var r = op.apply(this, arguments);
        if (r && r.then) r.then(null, function (er) { if (er && er.name === 'NotSupportedError') report(); });
        return r;
      };
    }
    // Media Source Extensions: addSourceBuffer throws for a type the engine cannot decode
    var M = window.MediaSource && MediaSource.prototype;
    if (M && M.addSourceBuffer) {
      var oa = M.addSourceBuffer;
      M.addSourceBuffer = function (t) {
        try { return oa.apply(this, arguments); }
        catch (er) { if (/avc|mp4a|hvc|hev|mp4/i.test(String(t))) report(); throw er; }
      };
    }
  } catch (e) {}
})();
