import { test } from 'node:test';
import assert from 'node:assert/strict';
import { lineIndex, wordProgress, wordFill, clock, latest, STALE } from '../js/lyricsmodel.js';

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
