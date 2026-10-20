// A small calculator: digits build the number, an operator remembers the left side, = finishes the sum.
(function () {
  var shown = '0', left = null, op = null, fresh = true;
  var out = document.getElementById('display');
  function show() { out.textContent = shown; }
  function apply(a, o, b) {
    if (o === '+') return a + b;
    if (o === '-') return a - b;
    if (o === '×') return a * b;
    if (o === '÷') return b === 0 ? NaN : a / b;
    return b;
  }
  function fmt(n) {
    if (!isFinite(n)) return 'Error';
    return String(Math.round(n * 1e9) / 1e9);
  }
  function press(k) {
    if (shown === 'Error' && k !== 'C') return;
    if (k === 'C') { shown = '0'; left = null; op = null; fresh = true; }
    else if (k >= '0' && k <= '9') { shown = fresh || shown === '0' ? k : shown + k; fresh = false; }
    else if (k === '.') { if (fresh) { shown = '0.'; fresh = false; } else if (shown.indexOf('.') < 0) shown += '.'; }
    else if (k === '=') { if (op !== null) { shown = fmt(apply(left, op, parseFloat(shown))); left = null; op = null; fresh = true; } }
    else {
      if (op !== null && !fresh) { shown = fmt(apply(left, op, parseFloat(shown))); }
      left = parseFloat(shown); op = k; fresh = true;
    }
    show();
  }
  var buttons = document.getElementsByTagName('button');
  for (var i = 0; i < buttons.length; i++) {
    (function (b) { b.addEventListener('click', function () { press(b.textContent.trim()); }); })(buttons[i]);
  }
})();
