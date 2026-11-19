// Keeps its list with iyakki.storage (this app asked for the storage capability, nothing else).
(function () {
  var items = [];
  var list = document.getElementById('list');
  var count = document.getElementById('count');
  function save() { iyakki.storage.set('items', JSON.stringify(items)); }
  function draw() {
    list.innerHTML = '';
    var open = 0;
    items.forEach(function (it, i) {
      var li = document.createElement('li');
      var t = document.createElement('span');
      t.textContent = it.text;
      if (it.done) t.className = 'done'; else open++;
      var b = document.createElement('button');
      b.textContent = it.done ? 'Undo ' + (i + 1) : 'Done ' + (i + 1);
      b.addEventListener('click', function () { it.done = !it.done; save(); draw(); });
      var d = document.createElement('button');
      d.textContent = 'Delete ' + (i + 1);
      d.addEventListener('click', function () { items.splice(i, 1); save(); draw(); });
      li.appendChild(t); li.appendChild(b); li.appendChild(d);
      list.appendChild(li);
    });
    count.textContent = open + ' to do, ' + (items.length - open) + ' done';
  }
  document.getElementById('addbtn').addEventListener('click', function () {
    var f = document.getElementById('text');
    var v = f.value.trim();
    if (!v) return;
    items.push({ text: v.slice(0, 200), done: false });
    f.value = '';
    save(); draw();
  });
  try { items = JSON.parse(iyakki.storage.get('items') || '[]'); } catch (e) { items = []; }
  draw();
})();
