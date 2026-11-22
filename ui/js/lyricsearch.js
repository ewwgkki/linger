// Manual lyrics search: online candidates or a local .lrc file for the playing song.
import { t } from './i18n.js';
import { api } from './api.js';
import { esc, mmss } from './util.js';
import { ly, subscribe, choose, useFile } from './lyrics.js';
import { pickStillValid } from './lyricsmodel.js';

const $ = (id) => document.getElementById(id);
let ctx = null;
/** song the dialog was opened for; results are never applied to another one */
let songId = null;
let results = [];
let busy = false;
/** numbers searches: only the newest one may show its result or error */
let searchSeq = 0;

const errText = (e) => {
  const k = String((e && e.message) || e);
  return t(k.startsWith('lyrics.') ? k : 'lyrics.netError');
};
const say = (msg, bad = false) => {
  $('lysmsg').textContent = msg;
  $('lysmsg').hidden = !msg;
  $('lysmsg').classList.toggle('bad', bad);
};
const sourceName = (s) => (s === 'netease' || s === 'lrclib' ? t(`lyrics.src.${s}`) : s);

function setBusy(on) {
  busy = on;
  for (const b of $('lysresults').querySelectorAll('button')) b.disabled = on;
  $('lysfile').disabled = on;
}

function showResults() {
  $('lysresults').innerHTML = results.map((r, i) => `<button type="button" class="lys-row" data-i="${i}">
    <span class="lys-main"><b>${esc(r.title)}</b><small>${esc([r.artist, r.album].filter(Boolean).join(' · '))}</small></span>
    <span class="lys-meta"><span class="lys-tag${r.synced ? ' on' : ''}">${esc(t(r.synced ? 'lyrics.synced' : 'lyrics.unsynced'))}</span><small>${esc(sourceName(r.source))} · ${esc(mmss(r.dur))}</small></span>
  </button>`).join('');
}

async function search() {
  const id = songId, me = ++searchSeq;
  if (!id) return;
  results = [];
  showResults();
  say(t('lyrics.searching'));
  let found;
  try {
    found = await api.invoke('lyrics_search', { id, title: $('lystitle').value.trim() || null, artist: $('lysartist').value.trim() || null });
  } catch (e) {
    if (me === searchSeq && songId === id) say(errText(e), true);
    return;
  }
  if (me !== searchSeq || songId !== id) return;
  results = found || [];
  showResults();
  say(results.length ? '' : t('lyrics.noResults'));
}

async function apply(fn) {
  if (busy) return;
  const id = songId;
  setBusy(true);
  say(t('lyrics.applying'));
  try {
    await fn();
    if (songId === id) $('lyricsdlg').close();
  } catch (e) {
    if (songId === id) say(errText(e), true);
  } finally {
    setBusy(false);
  }
}

async function pickFile() {
  const tauri = globalThis.__TAURI__;
  const opts = { multiple: false, directory: false, filters: [{ name: t('lyrics.pickFilter'), extensions: ['lrc', 'txt'] }] };
  if (tauri && tauri.dialog) return tauri.dialog.open(opts);
  if (tauri) return tauri.core.invoke('plugin:dialog|open', { options: opts });
  return prompt(t('lyrics.promptPath')); // browser preview: no native dialog
}

async function useLocal() {
  if (busy) return;
  const id = songId;
  // the picker stays open for as long as the user likes: the song may change meanwhile
  const valid = () => pickStillValid(id, ly.id, $('lyricsdlg').open && songId === id);
  let path;
  try {
    path = await pickFile();
  } catch (e) {
    console.error('lyrics file dialog', e);
    if (valid()) say(t('lyrics.badFile'), true);
    return;
  }
  if (!valid()) return;
  if (Array.isArray(path)) path = path[0];
  if (path) apply(() => useFile(path));
}

export function openLyricSearch() {
  if (!ly.id) return;
  const s = ctx.songsById.get(ly.id);
  songId = ly.id;
  $('lystitle').value = s ? s.t : '';
  $('lysartist').value = s ? s.a : '';
  const d = $('lyricsdlg');
  if (!d.open) d.showModal();
  $('lystitle').focus();
  search();
}

export function initLyricSearch(c) {
  ctx = c;
  $('lysform').addEventListener('submit', (e) => { e.preventDefault(); search(); });
  $('lysx').onclick = () => $('lyricsdlg').close();
  $('lysfile').onclick = useLocal;
  $('lysresults').addEventListener('click', (e) => {
    const b = e.target.closest('[data-i]');
    const r = b && results[+b.dataset.i];
    if (r) apply(() => choose(r.source, r.ext_id));
  });
  $('lyricsdlg').addEventListener('close', () => { songId = null; results = []; $('lysresults').textContent = ''; say(''); });
  // the song changed under the dialog: its results belong to the old one
  subscribe(() => { if (songId && ly.id !== songId && $('lyricsdlg').open) $('lyricsdlg').close(); });
}
