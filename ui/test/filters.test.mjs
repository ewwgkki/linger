import { test } from 'node:test';
import assert from 'node:assert/strict';
import { enterPage, isScopedPage, byPageQuality, pageQualityOptions } from '../js/filters.js';
import { selectSongs } from '../js/library.js';

const song = (id, r, c, br) => ({ id, t: id, a: 'A', b: 'B', n: 0, s: 1, c, f: c, br, z: 1, pc: 0, r, p: id, st: 0, av: true });
const lib = [song('l1', 1, 'flac', 900), song('s1', 1, 'mp3', 192), song('s2', 2, 'mp3', 192), song('h2', 2, 'mp3', 320)];
const base = () => ({ tab: 'songs', q: 'x', artist: 'A', album: 'B', quality: 'std', root: 1, pageQ: 'low', open: 's1', pl: null, sort: { col: 'a', dir: 1 } });
const ids = (l) => l.map((s) => s.id).sort();

test('entering a page clears every filter', () => {
  const s = enterPage(base(), { tab: 'artists' });
  assert.deepEqual([s.tab, s.q, s.artist, s.album, s.quality, s.root, s.pageQ, s.open], ['artists', '', null, null, null, null, null, null]);
});

test('the sidebar quality covers all folders', () => {
  const s = enterPage(base(), { tab: 'songs', quality: 'std' });
  assert.deepEqual(ids(selectSongs(lib, s)), ['s1', 's2']);
});

test('the page quality narrows the folder only', () => {
  const s = enterPage(base(), { tab: 'songs', root: 1 });
  assert.deepEqual(ids(selectSongs(lib, s)), ['l1', 's1']);
  s.pageQ = 'std';
  assert.deepEqual(ids(selectSongs(lib, s)), ['s1']);
});

test('scoped pages are folders, artists, albums and lists', () => {
  const p = (o) => ({ tab: 'songs', root: null, artist: null, album: null, quality: null, ...o });
  for (const o of [{ root: 1 }, { artist: '' }, { album: 'B' }, { tab: 'pl' }, { tab: 'fav' }]) assert.equal(isScopedPage(p(o)), true, JSON.stringify(o));
  for (const o of [{}, { quality: 'std' }, { tab: 'artists' }, { tab: 'albums' }]) assert.equal(isScopedPage(p(o)), false, JSON.stringify(o));
});

test('page quality never matches a missing playlist row', () => {
  const gone = { id: 'x', missing: true, t: 'x', a: '', b: '', s: 1 };
  assert.deepEqual(ids(byPageQuality([lib[1], gone], 'low')), []);
  assert.equal(byPageQuality([lib[1], gone], null).length, 2);
});

test('options list all plus the qualities present, in order', () => {
  assert.deepEqual(pageQualityOptions([lib[3], lib[0], lib[1], lib[2]]), [
    { k: null, n: 4 }, { k: 'lossless', n: 1 }, { k: 'high', n: 1 }, { k: 'std', n: 2 },
  ]);
});

test('no options when the page has a single quality', () => {
  assert.deepEqual(pageQualityOptions([lib[1], lib[2]]), []);
  assert.equal(pageQualityOptions([lib[1], lib[2]], 'std').length, 2, 'an active filter stays visible');
});
