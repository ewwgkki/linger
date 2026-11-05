import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pickerRows, cycle, diff } from '../js/savepicker.js';

test('pickerRows states', () => {
  assert.deepEqual(pickerRows({ total: 3, fav: 3, lists: [[1, 0], [2, 2]] }),
    [{ id: 0, state: 'on' }, { id: 1, state: 'off' }, { id: 2, state: 'some' }]);
});

test('pickerRows with nothing selected is all off', () => {
  assert.deepEqual(pickerRows({ total: 0, fav: 0, lists: [] }), [{ id: 0, state: 'off' }]);
});

test('cycle never returns to some', () => {
  assert.equal(cycle('some'), 'on');
  assert.equal(cycle('on'), 'off');
  assert.equal(cycle('off'), 'on');
});

test('diff only sends changes', () => {
  assert.deepEqual(diff({ 0: 'on', 1: 'off', 2: 'some', 3: 'some' }, { 0: 'off', 1: 'on', 2: 'some', 3: 'on' }), { add: [1, 3], remove: [0] });
  assert.deepEqual(diff({ 0: 'off', 5: 'some' }, { 0: 'off', 5: 'off', 6: 'on' }), { add: [6], remove: [5] });
});
