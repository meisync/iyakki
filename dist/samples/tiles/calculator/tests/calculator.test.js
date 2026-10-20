// Headless tests on the real engine: T.click / T.text drive the page like a person would.
function key(label) {
  var all = document.getElementsByTagName('button');
  for (var i = 0; i < all.length; i++) if (all[i].textContent.trim() === label) { all[i].click(); return; }
  throw new Error('no key ' + label);
}
T.test('7 + 8 = 15', function () {
  key('C'); key('7'); key('+'); key('8'); key('=');
  T.eq(T.text('#display'), '15');
});
T.test('division by zero says Error and C clears it', function () {
  key('C'); key('5'); key('÷'); key('0'); key('=');
  T.eq(T.text('#display'), 'Error');
  key('C');
  T.eq(T.text('#display'), '0');
});
T.test('decimals', function () {
  key('C'); key('1'); key('.'); key('5'); key('×'); key('2'); key('=');
  T.eq(T.text('#display'), '3');
});
