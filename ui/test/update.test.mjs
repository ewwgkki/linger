import test from 'node:test';
import assert from 'node:assert/strict';
import { initialUpdate, nextUpdate, hasUpdate } from '../js/update.js';

const run = (...evs) => evs.reduce(nextUpdate, initialUpdate());
const info = { version: '0.7.0', notes: 'New' };

test('full update path', () => {
  const s = run({ type: 'check' }, { type: 'found', info, manual: true }, { type: 'download' }, { type: 'progress', pct: 40 });
  assert.equal(s.phase, 'downloading');
  assert.equal(s.pct, 40);
  const ready = nextUpdate(s, { type: 'ready' });
  assert.equal(ready.phase, 'ready');
  assert.equal(nextUpdate(ready, { type: 'install' }).phase, 'installing');
});

test('background checks stay quiet', () => {
  assert.equal(run({ type: 'found', info: null, manual: false }).phase, 'idle');
  assert.equal(run({ type: 'checkFailed', manual: false, error: 'update.checkFailed' }).phase, 'idle');
});

test('manual checks report the result', () => {
  assert.equal(run({ type: 'check' }, { type: 'found', info: null, manual: true }).phase, 'latest');
  const s = run({ type: 'check' }, { type: 'checkFailed', manual: true, error: 'update.checkFailed' });
  assert.equal(s.phase, 'error');
  assert.equal(s.error, 'update.checkFailed');
});

test('late download events cannot revive a cancelled download', () => {
  const s = run({ type: 'found', info }, { type: 'download' }, { type: 'cancel' });
  assert.equal(nextUpdate(s, { type: 'ready' }).phase, 'available');
  assert.equal(nextUpdate(s, { type: 'failed' }).phase, 'available');
  assert.equal(nextUpdate(s, { type: 'progress', pct: 90 }).pct, 0);
});

test('a failed install can be retried', () => {
  const s = run({ type: 'found', info }, { type: 'download' }, { type: 'ready' }, { type: 'install' }, { type: 'installFailed', error: 'update.installFailed' });
  assert.equal(s.phase, 'ready');
  assert.equal(s.error, 'update.installFailed');
  assert.equal(hasUpdate(s), true);
});
