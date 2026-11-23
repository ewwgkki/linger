import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeSkin, currentSkin, setSkin } from '../js/skin.js';

const store = () => { const m = new Map(); return { m, setItem: (k, v) => m.set(k, v) }; };

test('unknown themes fall back to classic', () => {
  assert.equal(normalizeSkin('fresh'), 'fresh');
  assert.equal(normalizeSkin('neon'), 'classic');
  assert.equal(normalizeSkin(undefined), 'classic');
});

test('setSkin applies, saves and clears the theme', () => {
  const root = { dataset: {} }, s = store();
  assert.equal(setSkin('fresh', root, s), 'fresh');
  assert.equal(root.dataset.skin, 'fresh');
  assert.equal(s.m.get('linger.skin'), 'fresh');
  assert.equal(currentSkin(root), 'fresh');
  assert.equal(setSkin('classic', root, s), 'classic');
  assert.equal('skin' in root.dataset, false);
  assert.equal(s.m.get('linger.skin'), 'classic');
});

test('a storage failure still applies the theme', () => {
  const root = { dataset: {} };
  assert.equal(setSkin('fresh', root, { setItem() { throw new Error('blocked'); } }), 'fresh');
  assert.equal(root.dataset.skin, 'fresh');
});
