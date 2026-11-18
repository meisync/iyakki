(function (g) {
  'use strict';
  // The window the page is told it has comes from the embedder (the person's window), not from a fixed 1280 by 720.
  const V = JSON.parse(__host('window', 'viewport', '{}'));
  Object.assign(g, { innerWidth: V.w, innerHeight: V.h, outerWidth: V.w, outerHeight: V.h, devicePixelRatio: V.d > 0 ? V.d : 1 });
})(globalThis);
