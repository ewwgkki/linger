// App shell: boot, sidebar, routing between views, history navigation.
import { initI18n, applyI18n, t, lang } from './i18n.js';
import { api, isMock } from './api.js';
import { esc, playable, icon } from './util.js';
import { info } from './library.js';
import { renderSongs, QUALITIES, qLabel, rootLabel, fullPath, rowHeight } from './views/songs.js';
import { renderShelf } from './views/shelf.js';

const $ = (id) => document.getElementById(id);
const store = {
  get(k, d) { try { const v = localStorage.getItem(k); return v == null ? d : JSON.parse(v); } catch (_) { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (_) { /* ignore */ } },
};

const state = { tab: 'songs', q: '', artist: null, album: null, quality: null, root: null, sort: { col: 'a', dir: 1 }, open: null, shelfSort: 'count' };
/** Shared with views and (Task 11) the player / settings modules. */
export const ctx = {
  state,
  lib: { roots: [], songs: [], scanned_at: null, ffmpeg: false },
  caps: { ffmpeg: false },
  nowId: null,
  /** Songs currently shown on the songs page, in display order. */
  list: [],
  view: null,
  render: () => render(),
  /** Plays `list` (already filtered to playable songs) from `start`. */
  playList(list, start, shuffle) {
    if (!list.length) return;
    api.invoke('player_play_list', { ids: list.map((s) => s.id), start: Math.max(0, start), shuffle });
  },
  openSettings: () => { const d = $('settings'); closeDrawer(); if (!d.open) d.showModal(); },
};

// Covers that fail to load fall back to the symbol icon underneath (no inline handlers under the CSP).
document.addEventListener('error', (e) => { if (e.target.tagName === 'IMG' && e.target.closest('.art')) e.target.remove(); }, true);

// ---------- library ----------
async function loadLibrary() {
  const lib = await api.invoke('get_library');
  ctx.lib = { roots: lib.roots || [], songs: lib.songs || [], scanned_at: lib.scanned_at, ffmpeg: !!lib.ffmpeg };
  ctx.caps = { ffmpeg: !!lib.ffmpeg };
  if (state.root != null && !ctx.lib.roots.some((r) => r.id === state.root)) state.root = null;
  sidebar();
  render();
}

function sidebar() {
  const { songs, roots, scanned_at } = ctx.lib;
  const secs = songs.reduce((a, s) => a + s.s, 0), gb = songs.reduce((a, s) => a + s.z, 0) / 1073741824;
  const artists = new Set(songs.map((s) => s.a || '')).size;
  const albums = new Set(songs.map((s) => s.b).filter(Boolean)).size;
  $('libsum').textContent = songs.length ? t('side.summary', { n: songs.length, h: Math.round(secs / 3600), gb: gb.toFixed(1) }) : t('side.empty');
  $('libsum').title = $('libsum').textContent;
  $('n-songs').textContent = songs.length || '';
  $('n-artists').textContent = (songs.length && artists) || '';
  $('n-albums').textContent = albums || '';
  const cnt = Object.fromEntries(QUALITIES.map((q) => [q.k, 0]));
  for (const s of songs) cnt[info(s).q]++;
  $('qbar').innerHTML = songs.length ? QUALITIES.map((q) => `<i style="width:${cnt[q.k] / songs.length * 100}%;background:var(${q.color})" title="${esc(qLabel(q.k) + ' ' + t('meta.songs', { n: cnt[q.k] }))}"></i>`).join('') : '';
  $('qnav').innerHTML = QUALITIES.map((q) => `<button data-q="${q.k}" aria-pressed="false"><span class="dot" style="background:var(${q.color})"></span><span class="lbl">${esc(qLabel(q.k))}</span><span class="n">${cnt[q.k]}</span></button>`).join('');
  $('rootsec').hidden = roots.length < 2;
  $('rootnav').innerHTML = roots.length < 2 ? '' :
    `<button data-root="" aria-pressed="false">${icon('folder')}<span class="lbl">${esc(t('side.allFolders'))}</span><span class="n">${songs.length}</span></button>` +
    roots.map((r) => `<button data-root="${r.id}" aria-pressed="false" class="${r.online ? '' : 'off'}" title="${esc(r.path + (r.online ? '' : ' · ' + t('root.offline')))}">${icon(r.online ? 'folder' : 'cloud-off')}<span class="lbl">${esc(rootLabel(r))}</span><span class="n">${r.count}</span></button>`).join('');
  const offline = roots.filter((r) => !r.online).length;
  $('scanline').innerHTML = (scanned_at ? esc(t('side.lastScan', { time: new Date(scanned_at * 1000).toLocaleString(lang(), { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }) })) : '') +
    (offline ? `<div class="warn">${esc(t('side.offlineFolders', { n: offline }))}</div>` : '');
  syncSide();
}

function syncSide() {
  document.querySelectorAll('[data-tab]').forEach((b) => b.setAttribute(b.closest('#mtabs') ? 'aria-pressed' : 'aria-selected', b.dataset.tab === state.tab));
  document.querySelectorAll('#qnav button').forEach((b) => b.setAttribute('aria-pressed', b.dataset.q === state.quality));
  document.querySelectorAll('#rootnav button').forEach((b) => b.setAttribute('aria-pressed', b.dataset.root === (state.root == null ? '' : String(state.root))));
}

$('qnav').addEventListener('click', (e) => {
  const b = e.target.closest('button'); if (!b) return;
  state.quality = state.quality === b.dataset.q ? null : b.dataset.q; state.tab = 'songs'; state.open = null; closeDrawer(); render();
});
$('rootnav').addEventListener('click', (e) => {
  const b = e.target.closest('button'); if (!b) return;
  state.root = b.dataset.root === '' ? null : +b.dataset.root; state.open = null; closeDrawer(); render();
});
function closeDrawer() { $('side').classList.remove('open'); $('scrim').classList.remove('open'); }
$('menubtn').onclick = () => { $('side').classList.add('open'); $('scrim').classList.add('open'); };
$('scrim').onclick = closeDrawer;

// ---------- views ----------
function render() {
  syncSide();
  $('clearq').hidden = !state.q;
  if (ctx.view && ctx.view.vlist) ctx.view.vlist.destroy();
  if (state.tab === 'songs') {
    ctx.view = renderSongs($('view'), ctx);
    ctx.list = ctx.view.list;
  } else {
    renderShelf($('view'), ctx, state.tab);
    ctx.view = null;
  }
}

const playableList = () => ctx.list.filter((s) => playable(s, ctx.caps));
function toggleOpen(id) {
  state.open = state.open === id ? null : id;
  if (ctx.view) ctx.view.refresh();
}

$('view').addEventListener('click', (e) => {
  const tg = e.target;
  if (tg.closest('[data-open-settings]')) { ctx.openSettings(); return; }
  if (tg.closest('#playall')) { ctx.playList(playableList(), 0, false); return; }
  if (tg.closest('#shuffleall')) { const l = playableList(); ctx.playList(l, Math.floor(Math.random() * l.length), true); return; }
  const ss = tg.closest('#shelfsort [data-v]');
  if (ss) { state.shelfSort = ss.dataset.v; store.set('shelfSort', state.shelfSort); render(); return; }
  const cl = tg.closest('[data-clear]');
  if (cl) { const k = cl.dataset.clear; if (k === 'q') resetSearch(); else state[k] = null; state.open = null; render(); return; }
  const pb = tg.closest('[data-play]');
  if (pb) { e.stopPropagation(); const l = playableList(); ctx.playList(l, l.findIndex((s) => s.id === pb.dataset.play), false); return; }
  const a = tg.closest('[data-artist]'), b = tg.closest('[data-album]');
  if (a) { e.stopPropagation(); go({ artist: a.dataset.artist, album: null, sort: { col: 'b', dir: 1 } }); return; }
  if (b) { e.stopPropagation(); go({ album: b.dataset.album, artist: null, sort: { col: 'n', dir: 1 } }); return; }
  const srt = tg.closest('[data-sort]');
  if (srt) { const c = srt.dataset.sort; state.sort = { col: c, dir: state.sort.col === c ? -state.sort.dir : (c === 's' || c === 'br' ? -1 : 1) }; render(); return; }
  const cp = tg.closest('[data-copy]');
  if (cp) { copyPath(cp); return; }
  if (tg.closest('a,button')) return;
  const row = tg.closest('.song');
  if (row) toggleOpen(row.dataset.id);
});

function copyPath(btn) {
  const s = ctx.lib.songs.find((x) => x.id === btn.dataset.copy);
  if (!s) return;
  const done = () => { btn.textContent = t('detail.copied'); setTimeout(() => { btn.textContent = t('detail.copyPath'); }, 1500); };
  const fallback = () => {
    const el = document.getElementById('path-' + s.id); if (!el) return;
    const r = document.createRange(); r.selectNodeContents(el); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r);
    btn.textContent = t('detail.selected');
  };
  if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(fullPath(ctx.lib.roots, s)).then(done).catch(fallback);
  else fallback();
}

/** Enters an artist / album page; "back" is history navigation. */
function go(o) {
  try { history.pushState({ ...o, tab: 'songs' }, ''); } catch (_) { /* ignore */ }
  apply(o);
}
function apply(o) {
  state.artist = o.artist ?? null; state.album = o.album ?? null;
  if (o.sort) state.sort = o.sort;
  resetSearch(); state.tab = o.tab || 'songs'; state.open = null; render(); window.scrollTo({ top: 0 });
}
window.addEventListener('popstate', (e) => {
  const s = e.state || {};
  apply({ artist: s.artist, album: s.album, tab: s.tab || tabFromHash(), sort: s.sort || { col: 'a', dir: 1 } });
});
$('view').addEventListener('dblclick', (e) => {
  const row = e.target.closest('.song'); if (!row || e.target.closest('button,a')) return;
  const l = playableList(), i = l.findIndex((s) => s.id === row.dataset.id);
  if (i >= 0) ctx.playList(l, i, false);
});
$('view').addEventListener('keydown', (e) => {
  const row = e.target.closest('.song');
  if (row && e.target === row && e.key === 'Enter') {
    e.preventDefault();
    const id = row.dataset.id;
    toggleOpen(id);
    document.querySelector(`.song[data-id="${CSS.escape(id)}"]`)?.focus();
  }
});
function switchTab(tab) {
  state.tab = tab; state.artist = null; state.album = null; state.open = null;
  if (tab === 'songs') state.sort = { col: 'a', dir: 1 };
  try { history.pushState({ tab }, '', '#' + tab); } catch (_) { /* ignore */ }
  closeDrawer(); render(); window.scrollTo({ top: 0 });
}
document.querySelectorAll('#nav,#mtabs').forEach((el) => el.addEventListener('click', (e) => { const b = e.target.closest('[data-tab]'); if (b) switchTab(b.dataset.tab); }));
function resetSearch() { $('q').value = ''; state.q = ''; }
let qt;
$('q').addEventListener('input', (e) => { clearTimeout(qt); qt = setTimeout(() => { state.q = e.target.value; state.open = null; render(); }, 120); });
$('clearq').onclick = () => { resetSearch(); render(); $('q').focus(); };
document.addEventListener('keydown', (e) => {
  const typing = /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName) || $('settings').open;
  if (e.key === '/' && !typing) { e.preventDefault(); $('q').focus(); }
  if (e.key === 'Escape') closeDrawer();
});
window.addEventListener('scroll', () => $('topbar').classList.toggle('scrolled', scrollY > 4), { passive: true });
// Row height changes at the narrow breakpoint.
matchMedia('(max-width:760px)').addEventListener('change', () => { if (ctx.view) ctx.view.refresh(); });

// ---------- top bar actions (Task 11 adds progress UI and the settings dialog) ----------
$('refresh').onclick = () => {
  if (!ctx.lib.roots.length) { ctx.openSettings(); return; }
  $('refresh').disabled = true; $('refresh').classList.add('spin');
  api.invoke('start_scan');
};
$('opensettings').onclick = () => ctx.openSettings();

function tabFromHash() { const h = location.hash.slice(1); return ['songs', 'artists', 'albums'].includes(h) ? h : 'songs'; }

// ---------- boot ----------
async function boot() {
  const qs = new URLSearchParams(location.search);
  if (isMock && ['light', 'dark'].includes(qs.get('theme'))) document.documentElement.dataset.theme = qs.get('theme');
  await initI18n(isMock ? qs.get('lang') || undefined : undefined);
  applyI18n();
  state.shelfSort = store.get('shelfSort', 'count');
  state.tab = tabFromHash();
  api.listen('library://roots', (roots) => { ctx.lib.roots = roots; sidebar(); });
  api.listen('scan://finished', () => {
    $('refresh').disabled = false; $('refresh').classList.remove('spin');
    loadLibrary();
  });
  try {
    await loadLibrary();
  } catch (e) {
    $('view').innerHTML = `<div class="empty">${icon('disc')}<h2>${esc(t('error.loadTitle'))}</h2><p>${esc(String(e && e.message || e))}</p></div>`;
    return;
  }
}

export const ready = boot();
export { render, loadLibrary, sidebar, rowHeight };
