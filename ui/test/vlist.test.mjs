import { test } from 'node:test';
import assert from 'node:assert/strict';
import { computeRange } from '../js/vlist.js';

test('vlist_renders_bounded_rows', () => {
  const r = computeRange({ scrollTop: 360000, viewport: 800, rowH: 72, count: 10000, overscan: 6, expanded: null });
  assert.ok(r.end - r.start <= Math.ceil(800 / 72) + 12 + 1, `${r.start}..${r.end}`);
  assert.equal(r.padTop, r.start * 72);
  assert.equal(r.padTop + (r.end - r.start) * 72 + r.padBottom, 10000 * 72);
  assert.ok(r.start * 72 <= 360000 && r.end * 72 >= 360800);
});

test('top of list and empty list', () => {
  assert.deepEqual(computeRange({ scrollTop: 0, viewport: 720, rowH: 72, count: 3, overscan: 6, expanded: null }),
    { start: 0, end: 3, padTop: 0, padBottom: 0 });
  assert.deepEqual(computeRange({ scrollTop: 0, viewport: 720, rowH: 72, count: 0, overscan: 6, expanded: null }),
    { start: 0, end: 0, padTop: 0, padBottom: 0 });
  // scrolled above the list (negative) or far past it
  assert.equal(computeRange({ scrollTop: -500, viewport: 720, rowH: 72, count: 100, overscan: 2, expanded: null }).start, 0);
  const past = computeRange({ scrollTop: 1e7, viewport: 720, rowH: 72, count: 100, overscan: 2, expanded: null });
  assert.equal(past.end, 100);
  assert.ok(past.start <= past.end);
});

test('expanded row shifts following rows', () => {
  const expanded = { index: 10, extra: 56 };
  const opts = { viewport: 720, rowH: 72, count: 1000, overscan: 0, expanded };
  // rows before the expanded one are unaffected
  const a = computeRange({ ...opts, scrollTop: 5 * 72 });
  assert.equal(a.start, 5);
  assert.equal(a.padTop, 5 * 72);
  // rows after it start `extra` pixels later
  const b = computeRange({ ...opts, scrollTop: 20 * 72 + 56 });
  assert.equal(b.start, 20);
  assert.equal(b.padTop, 20 * 72 + 56);
  // inside the expanded detail area we are still on row 10
  const c = computeRange({ ...opts, scrollTop: 11 * 72 + 20 });
  assert.equal(c.start, 10);
  // total height includes the extra
  for (const r of [a, b, c]) {
    const rendered = (r.end - r.start) * 72 + (r.start <= 10 && r.end > 10 ? 56 : 0);
    assert.equal(r.padTop + rendered + r.padBottom, 1000 * 72 + 56);
  }
});
