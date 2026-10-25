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
