import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { t, setMessages, detectLang, lang } from '../js/i18n.js';

const ui = join(dirname(fileURLToPath(import.meta.url)), '..');
const load = (l) => JSON.parse(readFileSync(join(ui, 'i18n', `${l}.json`), 'utf8'));

test('zh-CN and en have identical key sets', () => {
  const zh = Object.keys(load('zh-CN')).sort();
  const en = Object.keys(load('en')).sort();
  assert.deepEqual(zh, en);
  assert.ok(zh.length > 50);
});

test('every value is a non-empty string and placeholders match', () => {
  const zh = load('zh-CN'), en = load('en');
  const ph = (s) => (s.match(/\{\w+\}/g) || []).sort().join();
  for (const k of Object.keys(zh)) {
    assert.equal(typeof zh[k], 'string', k);
    assert.ok(zh[k].length && en[k].length, k);
    assert.equal(ph(zh[k]), ph(en[k]), k);
  }
});

test('backend keys are present', () => {
  const zh = load('zh-CN');
  for (const k of ['skip.notaudio', 'skip.unreadable', 'skip.dberror', 'player.notFound', 'player.needFfmpeg',
    'player.unsupported', 'player.io', 'player.corrupt', 'player.noOutput', 'song.offline', 'meta.songs']) {
    assert.ok(k in zh, k);
  }
});

test('en has no CJK', () => {
  for (const [k, v] of Object.entries(load('en'))) assert.ok(!/[　-鿿＀-￯]/.test(v), `${k}: ${v}`);
});

test('t interpolates params', () => {
  setMessages('zh-CN', load('zh-CN'));
  assert.equal(lang(), 'zh-CN');
  assert.equal(t('meta.songs', { n: 3 }), '3 首');
  setMessages('en', load('en'));
  assert.equal(t('meta.songs', { n: 3 }), '3 songs');
  assert.equal(lang(), 'en');
});

test('t falls back to the key when missing', () => {
  setMessages('en', {});
  assert.equal(t('nope.missing'), 'nope.missing');
});

test('detectLang', () => {
  assert.equal(detectLang('zh-TW'), 'zh-CN');
  assert.equal(detectLang('zh'), 'zh-CN');
  assert.equal(detectLang('en-US'), 'en');
  assert.equal(detectLang('ja'), 'en');
  assert.equal(detectLang(undefined), 'en');
});

function walk(dir, out = []) {
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (p.endsWith('.js')) out.push(p);
  }
  return out;
}

test('no hardcoded CJK in ui/js', () => {
  const files = walk(join(ui, 'js'));
  assert.ok(files.length >= 8, `found ${files.length} files`);
  for (const f of files) {
    const lines = readFileSync(f, 'utf8').split('\n');
    lines.forEach((l, i) => assert.ok(!/[　-鿿＀-￯]/.test(l), `${f}:${i + 1}: ${l.trim()}`));
  }
});

test('index.html text comes from i18n', () => {
  const html = readFileSync(join(ui, 'index.html'), 'utf8');
  const keys = new Set(Object.keys(load('en')));
  const used = [...html.matchAll(/data-i18n(?:-[a-z-]+)?="([^"]+)"/g)].map((m) => m[1]);
  assert.ok(used.length > 10);
  for (const k of used) assert.ok(keys.has(k), `missing key ${k}`);
  const body = html.slice(html.indexOf('<body'));
  assert.ok(!/[　-鿿＀-￯]/.test(body), 'CJK in index.html body');
});

test('every literal t() key used in ui/js exists', () => {
  const keys = new Set(Object.keys(load('en')));
  let n = 0;
  for (const f of walk(join(ui, 'js'))) {
    for (const m of readFileSync(f, 'utf8').matchAll(/\bt\('([\w.]+\w)'/g)) {
      n++;
      assert.ok(keys.has(m[1]), `${f}: missing key ${m[1]}`);
    }
  }
  assert.ok(n > 50, `found ${n} uses`);
});
