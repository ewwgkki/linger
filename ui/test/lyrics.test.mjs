import { test } from 'node:test';
import assert from 'node:assert/strict';
import { lineIndex, wordProgress, wordFill, clock, latest, STALE, seekTarget, scrollGate, lineFill, pickStillValid, backdropColors } from '../js/lyricsmodel.js';

const L = (start, text = 'x') => ({ start, end: null, text, words: [], trans: null });

test('lineIndex finds current line', () => {
  const lines = [L(1000), L(3000), L(5000)];
  assert.equal(lineIndex(lines, 999), -1);
  assert.equal(lineIndex(lines, 1000), 0);
  assert.equal(lineIndex(lines, 2999), 0);
  assert.equal(lineIndex(lines, 3000), 1);
  assert.equal(lineIndex(lines, 9000), 2);
  assert.equal(lineIndex([L(null), L(null)], 5000), -1);
  assert.equal(lineIndex([], 5000), -1);
});

test('wordProgress and wordFill', () => {
  const w = { start: 1000, end: 2000, text: 'a' };
  assert.equal(wordFill(w, 1500), 0.5);
  assert.equal(wordFill(w, 500), 0);
  assert.equal(wordFill(w, 2500), 1);
  const line = { start: 1000, end: 3000, text: 'ab', words: [w, { start: 2000, end: 3000, text: 'b' }] };
  assert.equal(wordProgress(line, 1500), 0.25);
  assert.equal(wordProgress(line, 2500), 0.75);
  assert.equal(wordProgress(line, 0), 0);
  assert.equal(wordProgress(line, 9000), 1);
  assert.equal(wordProgress({ start: 1000, end: 3000, text: 'x', words: [] }, 2000), 0.5);
});

test('clock interpolates while playing', () => {
  const c = clock();
  c.set(10000, true, 1000);
  assert.equal(c.at(1500), 10500);
});

test('clock stops when paused', () => {
  const c = clock();
  c.set(10000, false, 1000);
  assert.equal(c.at(5000), 10000);
  c.set(10000, true, 5000);
  c.set(10200, false, 5200);
  assert.equal(c.at(9000), 10200);
});

test('clock follows a seek', () => {
  const c = clock();
  c.set(10000, true, 1000);
  c.at(1500);
  c.set(2000, true, 1600);
  assert.equal(c.at(1600), 2000);
  c.set(30000, true, 1700);
  assert.equal(c.at(1700), 30000);
});

test('clock does not jump back on small jitter', () => {
  const c = clock();
  c.set(10000, true, 1000);
  assert.equal(c.at(1500), 10500);
  c.set(10400, true, 1500); // report 100 ms behind what was shown
  assert.ok(c.at(1500) >= 10500);
  assert.ok(c.at(1800) >= 10500);
});

test('stale_response_is_ignored', async () => {
  const wrap = latest();
  const got = [];
  const slow = wrap(() => new Promise((r) => setTimeout(() => r('A'), 30)));
  const fast = wrap(() => Promise.resolve('B'));
  const cb = (v) => { if (v !== STALE) got.push(v); };
  await Promise.all([slow.then(cb), fast.then(cb)]);
  await new Promise((r) => setTimeout(r, 50));
  assert.deepEqual(got, ['B']);
});

test('clock seek snaps even slightly backwards; jitter does not', () => {
  const c = clock();
  c.set(10000, true, 1000);
  assert.equal(c.at(1500), 10500);
  c.seek(10200, true, 1500); // 300 ms back, a real seek
  assert.equal(c.at(1500), 10200);
  c.set(10150, true, 1500); // jitter
  assert.equal(c.at(1500), 10200);
  c.seek(0, false, 1600); // song change
  assert.equal(c.at(2600), 0);
});

test('store: setOffset accumulates, last wins, failure restores; choice beats slow get', async () => {
  const calls = [];
  let getResolve, failNext = false;
  const stub = {
      invoke: (cmd, a) => {
        calls.push([cmd, a]);
        if (cmd === 'lyrics_get') return new Promise((r) => { getResolve = () => r({ id: a.id, status: 'ok', kind: 'plain', lines: [], offset: 0 }); });
        if (cmd === 'lyrics_set_offset') return failNext ? Promise.reject('x') : Promise.resolve(a.ms);
        if (cmd === 'lyrics_choose') return Promise.resolve({ id: a.id, status: 'ok', kind: 'line', lines: [], offset: 0 });
        return Promise.resolve(null);
      },
  };
  const L = await import('../js/lyrics.js');
  L.useBackend(stub);
  L.setActive(true);
  L.onSong('s1');
  const pending = getResolve;
  await L.choose('netease', '1');
  pending(); // the slow get finishes after the choice
  await new Promise((r) => setTimeout(r, 5));
  assert.equal(L.ly.data.kind, 'line');
  const a = L.setOffset(500), b = L.setOffset(500), c = L.setOffset(-200);
  assert.equal(L.ly.data.offset, 800);
  await Promise.all([a, b, c]);
  assert.equal(L.ly.data.offset, 800);
  assert.deepEqual(calls.filter((x) => x[0] === 'lyrics_set_offset').map((x) => x[1].ms), [500, 1000, 800]);
  failNext = true;
  await L.setOffset(1000);
  assert.equal(L.ly.data.offset, 800);
  assert.equal(L.nowMs() >= 800, true);
  L.onSong(null);
});

test('seekTarget subtracts offset', () => {
  assert.equal(seekTarget({ start: 5000 }, 0), 5000);
  assert.equal(seekTarget({ start: 5000 }, 500), 4500); // positive offset = lyrics earlier
  assert.equal(seekTarget({ start: 5000 }, -500), 5500);
  assert.equal(seekTarget({ start: 200 }, 500), 0); // never below 0
  assert.equal(seekTarget({ start: null }, 0), null); // untimed line
});

test('autoScroll pauses after a manual scroll', () => {
  const g = scrollGate(4000);
  assert.equal(g.allowed(0), true);
  g.touch(1000);
  assert.equal(g.allowed(1000), false);
  assert.equal(g.allowed(4999), false);
  assert.equal(g.allowed(5000), true);
  g.touch(6000);
  g.clear(); // a click on a line follows at once
  assert.equal(g.allowed(6001), true);
});

test('lineFill uses the next line when a line has no end, capped', () => {
  const lines = [
    { start: 1000, end: null, words: [] },
    { start: 3000, end: 40000, words: [] },
    { start: 40000, end: null, words: [] },
  ];
  assert.equal(lineFill(lines, 0, 2000), 0.5);
  assert.equal(lineFill(lines, 1, 8000), 0.5); // a long gap fills over 10 s at most
  assert.equal(lineFill(lines, 1, 13000), 1);
  assert.equal(lineFill(lines, 2, 42500), 0.5); // last line: 5 s
  const w = [{ start: 0, end: 1000, words: [{ start: 0, end: 500, text: 'a' }, { start: 500, end: 1000, text: 'b' }] }];
  assert.equal(lineFill(w, 0, 250), 0.25);
});

test('store: holdView counts open views; reload refetches', async () => {
  const got = [];
  const L = await import('../js/lyrics.js');
  L.useBackend({ invoke: (cmd, a) => { if (cmd === 'lyrics_get') got.push(a.id); return Promise.resolve({ id: a && a.id, status: 'none', kind: 'plain', lines: [], offset: 0 }); } });
  L.onSong(null);
  L.setActive(false);
  const a = L.holdView(), b = L.holdView();
  a(); a(); // releasing twice counts once
  L.onSong('v1');
  assert.deepEqual(got, ['v1']); // the second view is still open
  await new Promise((r) => setTimeout(r, 2));
  L.reload();
  assert.deepEqual(got, ['v1', 'v1']);
  await new Promise((r) => setTimeout(r, 2));
  b();
  L.onSong('v2');
  assert.deepEqual(got, ['v1', 'v1']); // no view open: nothing fetched
  L.onSong(null);
});

test('store: a failed settings save restores the switch and rejects', async () => {
  const L = await import('../js/lyrics.js');
  L.useBackend({ invoke: (cmd) => (cmd === 'lyrics_set_settings' ? Promise.reject('x') : Promise.resolve(null)) });
  L.ly.netease = true; L.ly.trans = true;
  await assert.rejects(L.setNetease(false));
  assert.equal(L.ly.netease, true);
  await assert.rejects(L.setTrans(false));
  assert.equal(L.ly.trans, true);
});

test('pickStillValid: a picked file applies only to the song it was picked for', () => {
  assert.equal(pickStillValid('a', 'a', true), true);
  assert.equal(pickStillValid('a', 'b', true), false); // song changed while the picker was open
  assert.equal(pickStillValid('a', null, true), false); // playback stopped
  assert.equal(pickStillValid('a', 'a', false), false); // dialog closed meanwhile
  assert.equal(pickStillValid(null, null, true), false); // no song at all
});

const lumOf = (hex) => {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const contrast = (a, b) => {
  const [hi, lo] = [lumOf(a), lumOf(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

test('backdrop keeps text readable', () => {
  const colors = [[255, 255, 255], [0, 0, 0], [128, 128, 128], [255, 0, 0], [255, 255, 0]];
  for (const c of colors) {
    const b = backdropColors(c);
    for (const name of ['ink', 'mute', 'off']) {
      assert.match(String(b[name]), /^#[0-9a-f]{6}$/, `${c} ${name}`);
      assert.ok(contrast(b[name], b.top) >= 4.5, `${c} ${name} on top: ${contrast(b[name], b.top)}`);
      assert.ok(contrast(b[name], b.bottom) >= 4.5, `${c} ${name} on bottom: ${contrast(b[name], b.bottom)}`);
    }
    assert.match(b.top, /^#[0-9a-f]{6}$/);
    assert.match(b.bottom, /^#[0-9a-f]{6}$/);
    // the three text levels stay distinct: sung > other lines > unsung
    assert.ok(lumOf(b.ink) > lumOf(b.mute) && lumOf(b.mute) >= lumOf(b.off), `${c} text levels`);
  }
});

test('backdrop keeps the cover hue and has a default for no cover', () => {
  const red = backdropColors([220, 30, 30]).top;
  assert.ok(parseInt(red.slice(1, 3), 16) > parseInt(red.slice(3, 5), 16), red);
  for (const bad of [null, undefined, [1, 2], ['a', 'b', 'c']]) {
    const d = backdropColors(bad);
    assert.ok(contrast(d.ink, d.top) >= 4.5 && contrast(d.ink, d.bottom) >= 4.5);
  }
});
