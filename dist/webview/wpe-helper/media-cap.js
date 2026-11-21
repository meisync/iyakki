// The person's video ceiling and the tenant's decoder switch (F-BROX-18, F-BROX-20). The helper puts `window.__iyakkiCap={h:HEIGHT,c:CODECS};`
// in front of this file and sets the same object again in the live page when the choice changes; the wrappers read it at every call. They only say
// "not supported" to what the page asks about, so a site picks a smaller picture or another codec itself: nothing is cut off from a running
// stream and the original answer is passed on for everything else. h = 0 means no ceiling; c = 0 means the audio and video add-on's decoders
// (H.264, H.265, AAC, AC-3) are off.
(function () {
  if (!window.__iyakkiCap) window.__iyakkiCap = { h: 0, c: 1 };
  if (window.__iyakkiCapInstalled) return;
  window.__iyakkiCapInstalled = true;
  var ADDON = /(^|[ ,"';.])(avc[13]|hev1|hvc1|mp4a\.40|mp4a\.a5|mp4a\.a6|ac-3|ec-3|mp4v)/i;
  function tall(type) {
    var m = /height\s*=\s*"?(\d+)/i.exec(String(type));
    return m ? +m[1] : 0;
  }
  function blockedType(type) {
    var cap = window.__iyakkiCap || { h: 0, c: 1 };   // read at every call: the helper replaces the object when the choice changes
    type = String(type);
    if (cap.h > 0 && tall(type) > cap.h) return true;
    if (!cap.c && ADDON.test(type)) return true;
    return false;
  }
  try {
    if (window.MediaSource && MediaSource.isTypeSupported) {
      var its = MediaSource.isTypeSupported;
      MediaSource.isTypeSupported = function (t) { return blockedType(t) ? false : its.apply(this, arguments); };
    }
  } catch (e) {}
  try {
    var cpt = HTMLMediaElement.prototype.canPlayType;
    HTMLMediaElement.prototype.canPlayType = function (t) { return blockedType(t) ? '' : cpt.apply(this, arguments); };
  } catch (e) {}
  try {
    if (navigator.mediaCapabilities && navigator.mediaCapabilities.decodingInfo) {
      var mc = navigator.mediaCapabilities, di = mc.decodingInfo;
      mc.decodingInfo = function (cfg) {
        try {
          var cap = window.__iyakkiCap || { h: 0, c: 1 };
          var v = cfg && cfg.video, a = cfg && cfg.audio;
          var no = (v && cap.h > 0 && +v.height > cap.h) || (!cap.c && ((v && ADDON.test(String(v.contentType))) || (a && ADDON.test(String(a.contentType)))));
          if (no) return Promise.resolve({ supported: false, smooth: false, powerEfficient: false });
        } catch (e) {}
        return di.apply(this, arguments);
      };
    }
  } catch (e) {}
})();
