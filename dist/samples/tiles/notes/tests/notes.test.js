T.test('starts empty', function () { T.eq(T.text('#count'), '0 to do, 0 done'); });
T.test('adding keeps the note and clears the field', function () {
  T.type('#text', 'Buy bread'); T.click('#addbtn');
  T.eq(T.text('#count'), '1 to do, 0 done');
  T.eq(T.text('#list li span'), 'Buy bread');
});
T.test('it is kept in the app storage', function () {
  T.ok(iyakki.storage.get('items').indexOf('Buy bread') >= 0);
});
T.test('done and delete', function () {
  T.click('#list li button');
  T.eq(T.text('#count'), '0 to do, 1 done');
});
