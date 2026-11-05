import { test } from 'node:test';
import assert from 'node:assert/strict';
import { itemsToRows, saveResultKey, selectRows } from '../js/playlists.js';

test('itemsToRows keeps order and marks missing', () => {
  const a = { id: 'a', t: 'Yesterday', a: 'The Beatles', b: 'Help!', s: 125, av: true };
  const items = [{ i: 1, id: 'a', t: 'old', a: '', b: '', s: 1, ad: 5 }, { i: 2, id: 'zz', t: 'Gone', a: 'X', b: '', s: 10, ad: 6 }];
  const rows = itemsToRows(items, new Map([['a', a]]));
  assert.equal(rows.length, 2);
  assert.equal(rows[0].id, 'a');
  assert.equal(rows[0]._i, 1);
  assert.equal(rows[0]._ad, 5);
  assert.equal(rows[0].t, 'Yesterday');
  assert.equal(rows[0].a, 'The Beatles');
  assert.equal(rows[0].av, true);
  assert.ok(!rows[0].missing);
  assert.equal(a._i, undefined, 'library song object is not mutated');
  assert.deepEqual({ ...rows[1] }, { id: 'zz', t: 'Gone', a: 'X', b: '', s: 10, missing: true, _i: 2, _ad: 6 });
});

test('itemsToRows keeps duplicates of one song as separate rows', () => {
  const a = { id: 'a', t: 'A' };
  const rows = itemsToRows([{ i: 3, id: 'a' }, { i: 4, id: 'a' }], new Map([['a', a]]));
  assert.deepEqual(rows.map((r) => r._i), [3, 4]);
});

test('saveResultKey cases', () => {
  assert.deepEqual(saveResultKey({ added: 3, duplicate: 1, removed: 0 }), ['save.added', { n: 3 }]);
  assert.deepEqual(saveResultKey({ added: 0, duplicate: 0, removed: 2 }), ['save.removed', { n: 2 }]);
  assert.deepEqual(saveResultKey({ added: 1, duplicate: 0, removed: 2 }), ['save.changed', { n: 1, r: 2 }]);
  assert.deepEqual(saveResultKey({ added: 0, duplicate: 2, removed: 0 }), ['save.nothing', {}]);
});

test('selectRows: custom order, date added, search, column sorts', () => {
  const lib = new Map([
    ['a', { id: 'a', t: 'Yesterday', a: 'The Beatles', b: 'Help!', s: 125, c: 'mp3', br: 320, p: 'x/a.mp3' }],
    ['b', { id: 'b', t: 'C', a: 'B', b: '', s: 200, c: 'flac', br: 900, p: 'x/b.flac' }],
  ]);
  const rows = itemsToRows([{ i: 1, id: 'b', ad: 10 }, { i: 2, id: 'zz', t: 'Gone', a: 'X', b: '', s: 10, ad: 30 }, { i: 3, id: 'a', ad: 20 }], lib);
  const ids = (l) => l.map((r) => r.id);
  assert.deepEqual(ids(selectRows(rows, '', { col: 'pos', dir: 1 })), ['b', 'zz', 'a']);
  assert.deepEqual(ids(selectRows(rows, '', { col: 'pos', dir: -1 })), ['a', 'zz', 'b']);
  assert.deepEqual(ids(selectRows(rows, '', { col: 'ad', dir: -1 })), ['zz', 'a', 'b']);
  assert.deepEqual(ids(selectRows(rows, '', { col: 't', dir: 1 })), ['b', 'zz', 'a']);
  assert.deepEqual(ids(selectRows(rows, 'beat', { col: 'pos', dir: 1 })), ['a']);
  assert.deepEqual(ids(selectRows(rows, 'gone', { col: 'ad', dir: -1 })), ['zz']);
});
