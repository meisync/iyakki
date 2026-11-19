// Headless tests: they run on IYAKKI's own engine with `tile_test` (or `iyakki pkg tile-test .`). T.click, T.type, T.text, T.eq, T.ok.
T.test('starts with a prompt', function () { T.eq(T.text('#message'), 'Press the button.'); });
T.test('counts presses and keeps the count', function () {
  T.click('#go'); T.click('#go');
  T.eq(T.text('#message'), 'Pressed 2 times');
  T.eq(iyakki.storage.get('count'), '2');
});
