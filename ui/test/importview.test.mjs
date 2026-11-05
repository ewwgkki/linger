import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setMessages } from '../js/i18n.js';
import { missingText, summaryKey } from '../js/importview.js';

const zh = JSON.parse(readFileSync(new URL('../i18n/zh-CN.json', import.meta.url), 'utf8'));
const report = {
  playlists: [
    { source: 'a.csv', name: 'Road', id: 1, total: 4, matched: 2, missing: [{ t: '晴天', a: '周杰伦', b: '叶惠美' }, { t: 'Yesterday', a: 'The Beatles', b: '' }] },
    { source: 'b.csv', name: 'Done', id: 2, total: 2, matched: 2, missing: [] },
  ],
  failed: [],
};

test('missingText formats lists', () => {
  setMessages('zh-CN', zh);
  assert.equal(missingText(report), 'Road（匹配 2/4）\n晴天 - 周杰伦 · 叶惠美\nYesterday - The Beatles');
});

test('missingText empty when all found', () => {
  setMessages('zh-CN', zh);
  assert.equal(missingText({ playlists: [report.playlists[1]], failed: [] }), '');
});

test('summaryKey sums', () => {
  const r = { playlists: [...report.playlists, { source: 'c.csv', name: 'None', id: null, total: 3, matched: 0, missing: [] }], failed: [] };
  assert.deepEqual(summaryKey(r), ['import.summary', { lists: 2, matched: 4, total: 9 }]);
});
