import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setMessages } from '../js/i18n.js';
import { songFacts } from '../js/songinfo.js';

setMessages('zh-CN', JSON.parse(readFileSync(new URL('../i18n/zh-CN.json', import.meta.url), 'utf8')));
const song = (o) => ({ id: 'x', t: '晴天', a: '周杰伦', b: '叶惠美', n: 3, s: 269, c: 'flac', f: 'flac', br: 900, z: 31457280, ad: 1760000000, ...o });
const keys = (l) => l.map(([k]) => k);
const val = (l, k) => l.find(([x]) => x === k)[1];

test('details list every field in order', () => {
  const f = songFacts(song(), 'D:\\m\\a.flac');
  assert.deepEqual(keys(f), ['detail.fTitle', 'detail.fArtist', 'detail.fAlbum', 'detail.fTrack', 'detail.fFormat', 'detail.fQuality', 'detail.fDuration', 'detail.fSize', 'detail.fAdded', 'detail.fPath']);
  assert.equal(val(f, 'detail.fFormat'), 'FLAC');
  assert.equal(val(f, 'detail.fDuration'), '4:29');
  assert.equal(val(f, 'detail.fSize'), '30.0 MB');
  assert.equal(val(f, 'detail.fPath'), 'D:\\m\\a.flac');
  assert.match(val(f, 'detail.fQuality'), /900 kbps/);
});

test('empty fields are left out and an unknown artist is named', () => {
  const f = songFacts(song({ a: '', b: '', n: 0, ad: 0 }), 'p');
  assert.deepEqual(keys(f), ['detail.fTitle', 'detail.fArtist', 'detail.fFormat', 'detail.fQuality', 'detail.fDuration', 'detail.fSize', 'detail.fPath']);
  assert.equal(val(f, 'detail.fArtist'), '未知歌手');
});
