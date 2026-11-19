// Start here. The page is plain HTML, CSS and JavaScript; `iyakki` is the bridge to the system (see README.md).
// Use real <button>, <input>, <select> and <a> elements for anything the person clicks or types into.
(function () {
  var n = 0;
  try { n = parseInt(iyakki.storage.get('count') || '0', 10) || 0; } catch (e) { n = 0; }
  var msg = document.getElementById('message');
  function draw() { msg.textContent = n === 0 ? 'Press the button.' : 'Pressed ' + n + (n === 1 ? ' time' : ' times'); }
  document.getElementById('go').addEventListener('click', function () {
    n += 1;
    iyakki.storage.set('count', String(n));
    draw();
  });
  draw();
})();
