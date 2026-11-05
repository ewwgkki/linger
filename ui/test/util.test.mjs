import { test } from 'node:test';
import assert from 'node:assert/strict';
import { esc, mmss, norm, fmtLabel, quality, playable, coverUrl, hm, applyOnline, trackSlider, DRAG_END } from '../js/util.js';
import { setMessages } from '../js/i18n.js';

test('fmtLabel', () => {
  assert.equal(fmtLabel({ c: 'pcm', f: 'mp3' }), 'WAV');
  assert.equal(fmtLabel({ c: 'aac', f: 'm4a' }), 'M4A');
  assert.equal(fmtLabel({ c: 'aac', f: 'aac' }), 'AAC');
  assert.equal(fmtLabel({ c: 'flac', f: 'flac' }), 'FLAC');
  assert.equal(fmtLabel({ c: 'alac', f: 'm4a' }), 'ALAC');
  assert.equal(fmtLabel({ c: '', f: 'mp2' }), 'MP2');
});

test('quality tiers', () => {
  assert.equal(quality({ c: 'flac', br: 900 }), 'lossless');
  assert.equal(quality({ c: 'ape', br: 100 }), 'lossless');
  assert.equal(quality({ c: 'mp3', br: 320 }), 'high');
  assert.equal(quality({ c: 'mp3', br: 250 }), 'high');
  assert.equal(quality({ c: 'mp3', br: 192 }), 'std');
  assert.equal(quality({ c: 'mp3', br: 160 }), 'std');
  assert.equal(quality({ c: 'mp3', br: 128 }), 'low');
});

test('playable', () => {
  assert.equal(playable({ c: 'wma', av: true }, { ffmpeg: false }), false);
  assert.equal(playable({ c: 'ape', av: true }, { ffmpeg: false }), false);
  assert.equal(playable({ c: 'wma', av: true }, { ffmpeg: true }), true);
  assert.equal(playable({ c: 'mp3', av: false }, { ffmpeg: true }), false);
  assert.equal(playable({ c: 'flac', av: true }, { ffmpeg: false }), true);
});

test('opus needs FFmpeg (symphonia 0.5 has no Opus decoder)', () => {
  assert.equal(playable({ c: 'opus', f: 'opus', av: true }, { ffmpeg: false }), false);
  assert.equal(playable({ c: 'opus', f: 'ogg', av: true }, { ffmpeg: false }), false);
  assert.equal(playable({ c: 'opus', av: true }, { ffmpeg: true }), true);
  assert.equal(playable({ c: 'ogg', av: true }, { ffmpeg: false }), true);
});

test('applyOnline recomputes av from the folders online flags', () => {
  const songs = [{ id: 'a', r: 1, av: true }, { id: 'b', r: 2, av: true }, { id: 'c', r: 3, av: false }, { id: 'd', r: 9, av: true }];
  const roots = [{ id: 1, online: true }, { id: 2, online: false }, { id: 3, online: true }];
  assert.equal(applyOnline(songs, roots), true);
  assert.deepEqual(songs.map((s) => s.av), [true, false, true, true]); // r 9 not listed: untouched
  assert.equal(applyOnline(songs, roots), false, 'nothing changes the second time');
  assert.equal(applyOnline([], roots), false);
});

function fakeSlider(value) {
  const el = new EventTarget();
  el.value = String(value);
  el.fire = (type) => el.dispatchEvent(new Event(type));
  return el;
}

test('trackSlider: a drag back to its start value still ends (no change event)', () => {
  for (const ender of DRAG_END.filter((e) => e !== 'change')) {
    const el = fakeSlider(50), ends = [];
    const d = trackSlider(el, { preview() {}, end: (v) => ends.push(v), current: () => 50 });
    assert.equal(d.dragging(), false);
    el.value = '70'; el.fire('input');
    assert.equal(d.dragging(), true);
    el.value = '50'; el.fire('input');
    el.fire(ender); // the browser fires no `change` here
    assert.equal(d.dragging(), false, ender);
    assert.deepEqual(ends, [null], `${ender}: nothing to commit`);
  }
});

test('trackSlider: commits a changed value exactly once', () => {
  const el = fakeSlider(10), ends = [], previews = [];
  const d = trackSlider(el, { preview: (v) => previews.push(v), end: (v) => ends.push(v), current: () => 10 });
  el.value = '30'; el.fire('input');
  el.value = '40'; el.fire('input');
  el.fire('pointerup');
  el.fire('change');
  el.fire('blur');
  assert.deepEqual(previews, [30, 40]);
  assert.deepEqual(ends, [40]);
  assert.equal(d.dragging(), false);
  el.fire('keyup'); // no drag in progress: ignored
  assert.deepEqual(ends, [40]);
});

test('esc, mmss, norm as in the web version', () => {
  assert.equal(esc('<a href="x">&</a>'), '&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;');
  assert.equal(mmss(0), '0:00');
  assert.equal(mmss(65.9), '1:05');
  assert.equal(mmss(3725), '1:02:05');
  assert.equal(mmss(-3), '0:00');
  assert.equal(norm('ＡＢＣ Déf'), 'abc déf');
  assert.equal(norm(undefined), '');
});

test('hm goes through i18n', () => {
  setMessages('en', { 'time.hm': '{h} h {m} min', 'time.m': '{m} min' });
  assert.equal(hm(3 * 3600 + 20 * 60), '3 h 20 min');
  assert.equal(hm(125), '2 min');
});

test('coverUrl', () => {
  assert.equal(coverUrl({ id: 'ab', pc: 0, z: 1 }), null);
  globalThis.__TAURI__ = { core: { convertFileSrc: (p, proto) => `http://${proto}.localhost/${p}` } };
  try {
    assert.equal(coverUrl({ id: 'ab12', pc: 2, z: 345 }), 'http://cover.localhost/ab12?v=345-2');
  } finally {
    delete globalThis.__TAURI__;
  }
});
