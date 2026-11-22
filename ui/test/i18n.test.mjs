import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { t, setMessages, detectLang, lang, setLanguage, onLanguage } from '../js/i18n.js';

const ui = join(dirname(fileURLToPath(import.meta.url)), '..');
const load = (l) => JSON.parse(readFileSync(join(ui, 'i18n', `${l}.json`), 'utf8'));

const isOne = (k) => k.endsWith('_one');

test('zh-CN and en have identical key sets (en may add _one singulars)', () => {
  const zh = Object.keys(load('zh-CN')).sort();
  const en = Object.keys(load('en')).filter((k) => !isOne(k)).sort();
  assert.deepEqual(zh, en);
  assert.ok(zh.length > 50);
  assert.ok(!zh.some(isOne), 'zh-CN needs no _one keys');
});

test('every value is a non-empty string and placeholders match', () => {
  const zh = load('zh-CN'), en = load('en');
  const ph = (s) => (s.match(/\{\w+\}/g) || []).sort().join();
  for (const k of Object.keys(zh)) {
    assert.equal(typeof zh[k], 'string', k);
    assert.ok(zh[k].length && en[k].length, k);
    assert.equal(ph(zh[k]), ph(en[k]), k);
  }
  for (const k of Object.keys(en).filter(isOne)) {
    const base = k.slice(0, -4);
    assert.ok(base in en, `${k} has no base key`);
    assert.ok(ph(en[base]).includes('{n}'), `${base} has no {n}`);
    assert.equal(typeof en[k], 'string', k);
    assert.equal(ph(en[k]), ph(en[base]), k);
  }
});

test('t picks the _one variant only for n === 1 when it exists', () => {
  setMessages('en', load('en'));
  assert.equal(t('meta.songs', { n: 1 }), '1 song');
  assert.equal(t('meta.songs', { n: 0 }), '0 songs');
  assert.equal(t('meta.songs', { n: 2 }), '2 songs');
  assert.equal(t('import.summary', { n: 1, matched: 3, total: 4 }), 'Created 1 playlist, matched 3 of 4 songs.');
  assert.equal(t('save.changed', { n: 1, r: 2 }), 'Saved 1 song, removed 2');
  setMessages('en', { k: '{n} things' });
  assert.equal(t('k', { n: 1 }), '1 things'); // no _one: base key
  setMessages('zh-CN', load('zh-CN'));
  assert.equal(t('meta.songs', { n: 1 }), '1 首');
});

test('backend keys are present', () => {
  const zh = load('zh-CN');
  for (const k of ['skip.notaudio', 'skip.unreadable', 'skip.dberror', 'player.notFound', 'player.needFfmpeg',
    'player.unsupported', 'player.io', 'player.corrupt', 'player.noOutput', 'song.offline', 'meta.songs',
    'queue.badPart', 'queue.badIndex', 'import.unsupported', 'import.tooLarge', 'import.unreadable', 'import.notCsv',
    'import.missingColumns', 'import.badJson', 'import.empty', 'import.badZip']) {
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

test('English count strings read correctly for one', () => {
  const en = load('en');
  for (const [k, v] of Object.entries(en)) {
    assert.ok(!/\(s\)/.test(v), `${k}: ${v}`);
    if (k.endsWith('_one') || !v.includes('{n}') || !/\{n\}[^.]*?\b(songs|albums|artists|folders|files|playlists)\b/.test(v)) continue;
    assert.ok(k + '_one' in en, `${k} needs a singular`);
    assert.ok(!/\b(songs|albums|artists|folders|files|playlists)\b/.test(en[k + '_one'].split('{n}')[1].split(/[,.·]/)[0]), `${k}_one`);
  }
});

test('switching language installs the words before listeners run and saves nothing on failure', async () => {
  setMessages('zh-CN', { k: 'zh' });
  const seen = [];
  const off = onLanguage((l) => seen.push([l, t('k')]));
  assert.equal(await setLanguage('fr', async () => ({ k: 'fr' })), false);
  await assert.rejects(setLanguage('en', async () => { throw new Error('x'); }));
  assert.equal(lang(), 'zh-CN');
  assert.equal(await setLanguage('en', async () => ({ k: 'en' })), true);
  assert.deepEqual(seen, [['en', 'en']]);
  assert.equal(await setLanguage('en', async () => ({ k: 'again' })), false, 'already current');
  off();
  setMessages('zh-CN', load('zh-CN'));
});

test('a slower earlier switch cannot override a later one', async () => {
  setMessages('zh-CN', { k: 'zh' });
  let release;
  const slow = setLanguage('en', () => new Promise((r) => { release = r; }));
  assert.equal(await setLanguage('zh-CN', async () => ({ k: 'zh2' })), false, 'already current');
  release({ k: 'en' });
  assert.equal(await slow, false);
  assert.equal(lang(), 'zh-CN');
  setMessages('zh-CN', load('zh-CN'));
});

test('one failing listener does not stop the others', async () => {
  setMessages('zh-CN', { k: 'zh' });
  let ran = false;
  const realError = console.error; console.error = () => {};
  const a = onLanguage(() => { throw new Error('x'); }), b = onLanguage(() => { ran = true; });
  await setLanguage('en', async () => ({ k: 'en' }));
  console.error = realError; a(); b();
  assert.ok(ran);
  setMessages('zh-CN', load('zh-CN'));
});
