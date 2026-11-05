import { test } from 'node:test';
import assert from 'node:assert/strict';
import { selectSongs, groupShelf } from '../js/library.js';

let k = 0;
const song = (o) => ({ id: String(++k), t: 't', a: '', b: '', n: 0, s: 100, c: 'mp3', f: 'mp3', br: 320, z: 1, pc: 0, r: 1, p: 'x.mp3', st: 0, av: true, ...o });
const base = { q: '', artist: null, album: null, quality: null, root: null, sort: { col: 'a', dir: 1 } };
const ids = (l) => l.map((s) => s.id);

const lib = [
  song({ id: 'a1', t: 'Hello World', a: 'Adele', b: '25', n: 2 }),
  song({ id: 'a2', t: 'Skyfall', a: 'Adele', b: '25', n: 1, c: 'flac', f: 'flac', br: 900 }),
  song({ id: 'b1', t: '光年之外', a: '邓紫棋', b: '新的心跳', n: 3, br: 128 }),
  song({ id: 'b2', t: 'Ballad', a: 'BEYOND', b: '海阔天空', n: 1, av: false }),
  song({ id: 'c1', t: 'Untitled', a: '', b: '', r: 2 }),
];

test('search matches title/artist/album case-insensitively', () => {
  assert.deepEqual(ids(selectSongs(lib, { ...base, q: 'hello' })), ['a1']);
  assert.deepEqual(ids(selectSongs(lib, { ...base, q: 'ADELE' })).sort(), ['a1', 'a2']);
  assert.deepEqual(ids(selectSongs(lib, { ...base, q: '新的' })), ['b1']);
  assert.deepEqual(ids(selectSongs(lib, { ...base, q: 'beyond' })), ['b2']);
  assert.deepEqual(ids(selectSongs(lib, { ...base, q: '  skyFALL ' })), ['a2']);
});

test('album view sorts by track number', () => {
  const r = selectSongs(lib, { ...base, album: '25', sort: { col: 'n', dir: 1 } });
  assert.deepEqual(ids(r), ['a2', 'a1']);
  const d = selectSongs(lib, { ...base, album: '25', sort: { col: 'n', dir: -1 } });
  assert.deepEqual(ids(d), ['a1', 'a2']);
});

test('artist filter + quality filter combine', () => {
  assert.deepEqual(ids(selectSongs(lib, { ...base, artist: 'Adele', quality: 'lossless' })), ['a2']);
  assert.deepEqual(ids(selectSongs(lib, { ...base, artist: 'Adele', quality: 'high' })), ['a1']);
  assert.deepEqual(ids(selectSongs(lib, { ...base, quality: 'low' })), ['b1']);
  // '' selects the unknown artist
  assert.deepEqual(ids(selectSongs(lib, { ...base, artist: '' })), ['c1']);
});

test('root filter', () => {
  assert.deepEqual(ids(selectSongs(lib, { ...base, root: 2 })), ['c1']);
});

test('offline songs stay in results', () => {
  assert.ok(ids(selectSongs(lib, base)).includes('b2'));
});

test('default sort is by artist with unknown artist last', () => {
  const r = ids(selectSongs(lib, base));
  assert.equal(r[r.length - 1], 'c1');
  assert.deepEqual(r.slice(0, 2), ['a2', 'a1']); // Adele/25 tie broken by track number
});

test('duration and quality sorts', () => {
  const l = [song({ id: 'x', s: 10 }), song({ id: 'y', s: 30 }), song({ id: 'z', s: 20 })];
  assert.deepEqual(ids(selectSongs(l, { ...base, sort: { col: 's', dir: -1 } })), ['y', 'z', 'x']);
  const q = [song({ id: 'm', br: 320 }), song({ id: 'l', c: 'flac', br: 700 }), song({ id: 'o', br: 128 })];
  assert.deepEqual(ids(selectSongs(q, { ...base, sort: { col: 'br', dir: -1 } })), ['l', 'm', 'o']);
});

test('does not mutate the input', () => {
  const copy = lib.slice();
  selectSongs(lib, { ...base, sort: { col: 't', dir: -1 } });
  assert.deepEqual(lib, copy);
});

test('groupShelf artists by count then name, albums skip blanks', () => {
  const g = groupShelf(lib, 'artists', { ...base, shelfSort: 'count' });
  assert.equal(g[0].name, 'Adele');
  assert.equal(g[0].songs.length, 2);
  assert.ok(g.some((x) => x.name === '' && x.songs.length === 1)); // unknown artist group
  const al = groupShelf(lib, 'albums', { ...base, shelfSort: 'name' });
  assert.ok(!al.some((x) => x.name === ''));
  assert.equal(al.length, 3);
});

test('groupShelf honours search, quality and root', () => {
  assert.deepEqual(groupShelf(lib, 'albums', { ...base, q: 'skyfall' }).map((g) => g.name), ['25']);
  assert.deepEqual(groupShelf(lib, 'artists', { ...base, quality: 'low' }).map((g) => g.name), ['邓紫棋']);
  assert.deepEqual(groupShelf(lib, 'artists', { ...base, root: 2 }).map((g) => g.name), ['']);
});
