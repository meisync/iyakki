T.test('metres to kilometres', function () {
  T.type('#value', '2500'); T.click('#go');
  T.eq(T.text('#result'), '2.5 km');
});
T.test('mixing families is refused in words', function () {
  var f = T.$('#from'); f.value = 'kg'; T.click('#go');
  T.eq(T.text('#result'), 'Those units do not match');
});
T.test('temperature', function () {
  T.eq(Math.round(window.__convert(100, 'C', 'F')), 212);
});
