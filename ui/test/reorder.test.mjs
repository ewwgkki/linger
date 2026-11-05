import { test } from 'node:test';
import assert from 'node:assert/strict';
import { dropIndex } from '../js/reorder.js';

test('stays until half row crossed', () => {
  assert.equal(dropIndex(3 * 72 + 30, 72, 10, 3), 3);
  assert.equal(dropIndex(4 * 72 + 35, 72, 10, 3), 3);
  assert.equal(dropIndex(4 * 72 + 37, 72, 10, 3), 4);
});

test('upward symmetric', () => {
  assert.equal(dropIndex(2 * 72 + 37, 72, 10, 3), 3);
  assert.equal(dropIndex(2 * 72 + 35, 72, 10, 3), 2);
});

test('clamps', () => {
  assert.equal(dropIndex(-50, 72, 10, 3), 0);
  assert.equal(dropIndex(9999, 72, 10, 3), 9);
});

test('far moves and edge rows', () => {
  assert.equal(dropIndex(8 * 72 + 40, 72, 10, 3), 8);
  assert.equal(dropIndex(0, 72, 10, 3), 0);
  assert.equal(dropIndex(5, 72, 1, 0), 0);
  assert.equal(dropIndex(9 * 72 + 10, 72, 10, 9), 9);
});
