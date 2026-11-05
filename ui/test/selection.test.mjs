import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createSelection, clickSelect, selectAll, selectedInOrder, pruneSelection } from '../js/selection.js';

const L = ['a', 'b', 'c', 'd'];
const ids = (sel) => [...sel.ids].sort();

test('plain click selects only', () => {
  const s = createSelection();
  clickSelect(s, L, 'a', { ctrl: true });
  clickSelect(s, L, 'c', { ctrl: true });
  clickSelect(s, L, 'b', {});
  assert.deepEqual(ids(s), ['b']);
  assert.equal(s.anchor, 'b');
});

test('ctrl toggles', () => {
  const s = createSelection();
  clickSelect(s, L, 'a', { ctrl: true });
  clickSelect(s, L, 'c', { ctrl: true });
  clickSelect(s, L, 'a', { ctrl: true });
  assert.deepEqual(ids(s), ['c']);
});

test('shift selects range from anchor', () => {
  const s = createSelection();
  clickSelect(s, L, 'b', {});
  clickSelect(s, L, 'd', { shift: true });
  assert.deepEqual(ids(s), ['b', 'c', 'd']);
  clickSelect(s, L, 'a', { shift: true });
  assert.deepEqual(ids(s), ['a', 'b']);
  assert.equal(s.anchor, 'b');
});

test('ctrl+shift unions', () => {
  const s = createSelection();
  clickSelect(s, L, 'a', {});
  clickSelect(s, L, 'd', { ctrl: true });
  assert.equal(s.anchor, 'd');
  clickSelect(s, L, 'b', { ctrl: true, shift: true });
  assert.deepEqual(ids(s), ['a', 'b', 'c', 'd']);
  assert.equal(s.anchor, 'd');
});

test('shift without an anchor selects only the row', () => {
  const s = createSelection();
  clickSelect(s, L, 'c', { shift: true });
  assert.deepEqual(ids(s), ['c']);
  assert.equal(s.anchor, 'c');
});

test('selectedInOrder follows list order', () => {
  const s = createSelection();
  clickSelect(s, L, 'd', { ctrl: true });
  clickSelect(s, L, 'a', { ctrl: true });
  assert.deepEqual(selectedInOrder(s, L), ['a', 'd']);
});

test('selectAll selects the whole list', () => {
  const s = createSelection();
  selectAll(s, L);
  assert.deepEqual(selectedInOrder(s, L), L);
});

test('prune drops hidden', () => {
  const s = createSelection();
  clickSelect(s, ['a', 'x'], 'a', {});
  clickSelect(s, ['a', 'x'], 'x', { ctrl: true });
  pruneSelection(s, ['a', 'b']);
  assert.deepEqual(ids(s), ['a']);
  assert.equal(s.anchor, null);
  clickSelect(s, ['a', 'b'], 'a', {});
  pruneSelection(s, ['a', 'b']);
  assert.equal(s.anchor, 'a');
});
