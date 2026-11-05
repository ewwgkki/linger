// App shell: boot, sidebar, routing between views, history navigation.
import { initI18n, applyI18n, t, lang } from './i18n.js';
import { api, isMock } from './api.js';
import { esc, playable, icon } from './util.js';
import { info } from './library.js';
import { renderSongs, QUALITIES, qLabel, rootLabel, fullPath, rowHeight } from './views/songs.js';
import { renderShelf } from './views/shelf.js';
import { renderPlaylist, defaultPlSort } from './views/playlist.js';
import { initPlayer, playList, markRows, libraryChanged, showSaved } from './player.js';
import { initSettings, openSettings, toast } from './settings.js';
import { pl, FAV, initPlaylists, loadItems, createPlaylist, renamePlaylist, deletePlaylist, errKey } from './playlists.js';
import { askName, confirmBox } from './dialogs.js';
import { createSelection, clickSelect, selectAll, selectedInOrder, pruneSelection, clearSelection } from './selection.js';
import { openSongMenu, closeMenus, playIds, queueIds, removeIds, savableIds } from './menu.js';
import { saveSongs } from './savepicker.js';

const $ = (id) => document.getElementById(id);
const store = {
  get(k, d) { try { const v = localStorage.getItem(k); return v == null ? d : JSON.parse(v); } catch (_) { return d; } },
  set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (_) { /* ignore */ } },
};

// tab: songs | artists | albums | fav | pl (with pl = playlist id); plSort is the sort on fav / pl pages.
const state = { tab: 'songs', q: '', artist: null, album: null, quality: null, root: null, sort: { col: 'a', dir: 1 }, open: null, shelfSort: 'count', pl: null, plSort: null };
const isPlTab = (tab) => tab === 'fav' || tab === 'pl';
/** Shared with views and the player / settings modules. */
export const ctx = {
  state,
  lib: { roots: [], songs: [], scanned_at: null, ffmpeg: false },
  /** Library songs by id, rebuilt on every library load. */
  songsById: new Map(),
  caps: { ffmpeg: false },
  nowId: null,
  /** Songs currently shown on the songs page, in display order. */
  list: [],
  view: null,
  /** True once the first get_library has returned. */
  loaded: false,
  /** Selected rows of the current list (song ids), kept across re-renders. */
  sel: createSelection(),
  render: () => render(),
  sidebar: () => sidebar(),
  /** Plays `list` (already filtered to playable songs) from `start`. */
  playList: (list, start, shuffle) => playList(list, start, shuffle),
  openSettings: () => { closeDrawer(); openSettings(); },
  openPlaylist: (id) => openPlaylist(id),
  goArtist: (a) => go({ artist: a, album: null, sort: { col: 'b', dir: 1 } }),
  goAlbum: (b) => go({ album: b, artist: null, sort: { col: 'n', dir: 1 } }),
  /** After a `playlists://changed` (data already re-fetched): update the sidebar and the open page. */
  playlistsChanged: (c) => playlistsChanged(c),
  /** Goes back to the page with the given filters (where a queue was started). */
  showSource(c) {
    if (isPlTab(c.tab)) {
      if (c.tab === 'pl' && !pl.list.some((p) => p.id === c.pl)) return;
      try { history.pushState({ tab: c.tab, pl: c.pl }, '', plHash(c.tab, c.pl)); } catch (_) { /* ignore */ }
      Object.assign(state, { tab: c.tab, pl: c.pl, artist: null, album: null, plSort: c.plSort ? { ...c.plSort } : defaultPlSort(c.tab), q: c.q, open: null });
      $('q').value = c.q;
      render();
      return;
    }
    try { history.pushState({ artist: c.artist, album: c.album, sort: c.sort, tab: 'songs' }, '', location.pathname); } catch (_) { /* ignore */ }
    Object.assign(state, { tab: 'songs', artist: c.artist, album: c.album, sort: { ...c.sort }, quality: c.quality, root: c.root, q: c.q, open: null });
    $('q').value = c.q;
    render();
  },
};

// Covers that fail to load fall back to the symbol icon underneath (no inline handlers under the CSP).
document.addEventListener('error', (e) => { if (e.target.tagName === 'IMG' && e.target.closest('.art')) e.target.remove(); }, true);

// ---------- library ----------
async function loadLibrary() {
  const lib = await api.invoke('get_library');
  ctx.lib = { roots: lib.roots || [], songs: lib.songs || [], scanned_at: lib.scanned_at, ffmpeg: !!lib.ffmpeg };
  ctx.songsById = new Map(ctx.lib.songs.map((s) => [s.id, s]));
  ctx.caps = { ffmpeg: !!lib.ffmpeg };
  ctx.loaded = true;
  if (state.root != null && !ctx.lib.roots.some((r) => r.id === state.root)) state.root = null;
  sidebar();
  render();
  libraryChanged();
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
  sidebarPl();
}

/** Liked-songs count and the playlist list. */
function sidebarPl() {
  $('n-fav').textContent = pl.favItems.length || '';
  $('plnav').innerHTML = pl.list.length
    ? pl.list.map((p) => `<button type="button" data-pl="${p.id}" title="${esc(p.name)}">${icon('list')}<span class="lbl">${esc(p.name)}</span><span class="n">${p.count}</span></button>`).join('')
    : pl.loaded ? `<p class="side-empty">${esc(t('side.noPlaylists'))}</p><button type="button" class="side-link" data-import>${esc(t('side.importSpotify'))}</button>` : '';
  syncSide();
}

function syncSide() {
  document.querySelectorAll('[data-tab]').forEach((b) => b.setAttribute(b.closest('#mtabs') ? 'aria-pressed' : 'aria-selected', b.dataset.tab === state.tab));
  document.querySelectorAll('#qnav button').forEach((b) => b.setAttribute('aria-pressed', b.dataset.q === state.quality));
  document.querySelectorAll('#rootnav button').forEach((b) => b.setAttribute('aria-pressed', b.dataset.root === (state.root == null ? '' : String(state.root))));
  document.querySelectorAll('#plnav [data-pl]').forEach((b) => {
    if (state.tab === 'pl' && +b.dataset.pl === state.pl) b.setAttribute('aria-current', 'page');
    else b.removeAttribute('aria-current');
  });
}

$('qnav').addEventListener('click', (e) => {
  const b = e.target.closest('button'); if (!b) return;
  state.quality = state.quality === b.dataset.q ? null : b.dataset.q; state.tab = 'songs'; state.open = null; closeDrawer(); render();
});
$('rootnav').addEventListener('click', (e) => {
  const b = e.target.closest('button'); if (!b) return;
  state.root = b.dataset.root === '' ? null : +b.dataset.root; state.open = null; closeDrawer(); render();
});
$('plnav').addEventListener('click', (e) => {
  const b = e.target.closest('[data-pl]');
  if (b) { openPlaylist(+b.dataset.pl); return; }
  if (e.target.closest('[data-import]')) { closeDrawer(); if (ctx.openImport) ctx.openImport(); else ctx.openSettings(); }
});
$('newpl').onclick = () => newPlaylist();
function closeDrawer() { $('side').classList.remove('open'); $('scrim').classList.remove('open'); }
$('menubtn').onclick = () => { $('side').classList.add('open'); $('scrim').classList.add('open'); };
$('scrim').onclick = closeDrawer;

// ---------- views ----------
function render() {
  syncSide();
  $('clearq').hidden = !state.q;
  if (ctx.view && ctx.view.vlist) ctx.view.vlist.destroy();
  if (state.tab === 'songs' || isPlTab(state.tab)) {
    ctx.view = (state.tab === 'songs' ? renderSongs : renderPlaylist)($('view'), ctx);
    ctx.list = ctx.view.list;
    markRows();
  } else {
    renderShelf($('view'), ctx, state.tab);
    ctx.view = null;
    ctx.list = [];
  }
  pruneSelection(ctx.sel, listIds());
  paintSel();
}

// ---------- selection ----------
const listIds = () => ctx.list.map((s) => s.id);
const selIds = () => selectedInOrder(ctx.sel, listIds());
/** Playlist item ids of the selected rows (playlist pages). */
const itemIdsOf = (ids) => { const set = new Set(ids); return ctx.list.filter((s) => set.has(s.id) && s._i != null).map((s) => s._i); };

/** Marks the selected rows that are rendered and shows the selection bar for 2+ songs. */
function paintSel() {
  document.querySelectorAll('#songlist .song').forEach((r) => r.classList.toggle('sel', ctx.sel.ids.has(r.dataset.id)));
  const n = ctx.sel.ids.size;
  $('selbar').hidden = n < 2;
  if (n < 2) return;
  $('seln').textContent = t('sel.count', { n });
  $('selremove').hidden = !isPlTab(state.tab);
  $('selremove').title = $('selremovetxt').textContent = t(state.tab === 'pl' ? 'menu.removeFromPl' : 'menu.removeFromFav');
}

function clearSel() {
  if (!ctx.sel.ids.size && ctx.sel.anchor == null) return;
  clearSelection(ctx.sel);
  paintSel();
}

$('selbar').addEventListener('click', (e) => {
  const b = e.target.closest('[data-sel]'); if (!b) return;
  const ids = selIds(), act = b.dataset.sel;
  if (act === 'play') playIds(ids, ctx);
  else if (act === 'next' || act === 'queue') queueIds(ids, act === 'next', ctx);
  else if (act === 'save') saveSongs(savableIds(ids, ctx), b, ctx);
  else if (act === 'remove') removeIds(ids, ctx, state.tab === 'pl' ? itemIdsOf(ids) : undefined);
  else if (act === 'clear') clearSel();
});

/** Context menu for `row` at (x, y): the whole selection when the row is in it, else just this row. */
function rowMenu(row, x, y) {
  const id = row.dataset.id;
  if (!ctx.sel.ids.has(id)) { clickSelect(ctx.sel, listIds(), id, {}); paintSel(); }
  const ids = selIds();
  openSongMenu(x, y, ids, ctx, { itemIds: state.tab === 'pl' ? itemIdsOf(ids) : undefined });
}

/** Set by a long press so the click that follows it is ignored. */
let pressedAt = -Infinity;
$('view').addEventListener('contextmenu', (e) => {
  const row = e.target.closest('.song');
  if (!row || e.target.closest('input,textarea')) return;
  e.preventDefault();
  if (performance.now() - pressedAt < 1000) return; // already opened by the long press
  let { clientX: x, clientY: y } = e;
  if (!x && !y) { const r = row.getBoundingClientRect(); x = r.left + 80; y = r.bottom - 8; } // keyboard (menu key, Shift+F10)
  rowMenu(row, x, y);
});
// Long press (touch, pen) = right click.
let press = null;
$('view').addEventListener('pointerdown', (e) => {
  if (e.pointerType === 'mouse') return;
  const row = e.target.closest('.song');
  if (!row || e.target.closest('button,a')) return;
  const p = { x: e.clientX, y: e.clientY, timer: setTimeout(() => { press = null; pressedAt = performance.now(); rowMenu(row, p.x, p.y); }, 500) };
  press = p;
});
const cancelPress = () => { if (press) { clearTimeout(press.timer); press = null; } };
$('view').addEventListener('pointermove', (e) => { if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 10) cancelPress(); });
['pointerup', 'pointercancel'].forEach((ev) => $('view').addEventListener(ev, cancelPress));

const playableList = () => ctx.list.filter((s) => playable(s, ctx.caps));
function toggleOpen(id) {
  state.open = state.open === id ? null : id;
  if (ctx.view) ctx.view.refresh();
}

$('view').addEventListener('click', (e) => {
  const tg = e.target;
  if (tg.closest('[data-open-settings]')) { ctx.openSettings(); return; }
  const tgo = tg.closest('[data-tab-go]');
  if (tgo) { switchTab(tgo.dataset.tabGo); return; }
  const rn = tg.closest('[data-pl-rename]');
  if (rn) { renameList(+rn.dataset.plRename); return; }
  const dl = tg.closest('[data-pl-delete]');
  if (dl) { removeList(+dl.dataset.plDelete); return; }
  const ps = tg.closest('[data-psort]');
  if (ps) {
    const c = ps.dataset.psort, cur = state.plSort || defaultPlSort(state.tab);
    state.plSort = { col: c, dir: cur.col === c ? -cur.dir : (c === 's' || c === 'br' || c === 'ad' ? -1 : 1) };
    render(); return;
  }
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
  const hb = tg.closest('[data-heart]');
  if (hb) { saveSongs([hb.dataset.heart], hb, ctx); return; }
  if (tg.closest('a,button')) return;
  const row = tg.closest('.song');
  if (!row) return;
  if (performance.now() - pressedAt < 1000) { pressedAt = -Infinity; return; } // the end of a long press
  const id = row.dataset.id, ctrl = e.ctrlKey || e.metaKey, shift = e.shiftKey;
  clickSelect(ctx.sel, listIds(), id, { ctrl, shift });
  if (!ctrl && !shift) toggleOpen(id);
  paintSel();
});
// Shift+click selects a range: keep the browser from selecting text too.
$('view').addEventListener('mousedown', (e) => { if (e.shiftKey && e.target.closest('.song')) e.preventDefault(); });

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
  resetSearch(); state.tab = o.tab || 'songs'; state.open = null;
  state.pl = state.tab === 'pl' ? o.pl ?? null : null;
  state.plSort = isPlTab(state.tab) ? defaultPlSort(state.tab) : null;
  render(); window.scrollTo({ top: 0 });
}
window.addEventListener('popstate', (e) => {
  const s = e.state || {}, h = fromHash();
  apply({ artist: s.artist, album: s.album, tab: s.tab || h.tab, pl: s.tab ? s.pl : h.pl, sort: s.sort || { col: 'a', dir: 1 } });
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
/** `replace`: replace the history entry (leaving a page that no longer exists). */
function switchTab(tab, replace = false) {
  state.tab = tab; state.artist = null; state.album = null; state.open = null; state.pl = null;
  state.plSort = isPlTab(tab) ? defaultPlSort(tab) : null;
  if (tab === 'songs') state.sort = { col: 'a', dir: 1 };
  try { history[replace ? 'replaceState' : 'pushState']({ tab }, '', '#' + tab); } catch (_) { /* ignore */ }
  closeDrawer(); render(); window.scrollTo({ top: 0 });
}
const plHash = (tab, id) => (tab === 'pl' ? '#pl/' + id : '#' + tab);
function openPlaylist(id) {
  Object.assign(state, { tab: 'pl', pl: id, artist: null, album: null, open: null, plSort: defaultPlSort('pl') });
  try { history.pushState({ tab: 'pl', pl: id }, '', plHash('pl', id)); } catch (_) { /* ignore */ }
  closeDrawer(); render(); window.scrollTo({ top: 0 });
}

// ---------- playlists: create, rename, delete, change events ----------
const showErr = (e) => toast(t(errKey(e)), { icon: 'x', autoHide: 6000 });

async function newPlaylist() {
  closeDrawer();
  let made = null;
  const name = await askName({ title: t('playlist.newTitle'), ok: t('dlg.create'), run: async (n) => { made = await createPlaylist(n, []); } });
  if (name != null && made) openPlaylist(made.id);
}

async function renameList(id) {
  const p = pl.list.find((x) => x.id === id);
  if (!p) return;
  await askName({ title: t('playlist.renameTitle'), value: p.name, ok: t('dlg.save'), run: (n) => renamePlaylist(id, n) });
}

async function removeList(id) {
  const p = pl.list.find((x) => x.id === id);
  if (!p) return;
  const yes = await confirmBox({ title: t('playlist.deleteTitle'), text: t('playlist.deleteText', { name: p.name }), ok: t('playlist.delete'), danger: true });
  if (!yes) return;
  try {
    await deletePlaylist(id);
  } catch (e) { showErr(e); return; }
  if (state.tab === 'pl' && state.pl === id) switchTab('songs', true);
  toast(t('playlist.deleted', { name: p.name }), { icon: 'check', autoHide: 3000 });
}

/** Playlist ids seen so far, to tell "deleted" from "never existed" for the open page. */
let knownPl = new Set();
function playlistsChanged(c) {
  const had = knownPl;
  knownPl = new Set(pl.list.map((p) => p.id));
  sidebarPl();
  showSaved();
  if (ctx.view) ctx.view.refresh(); // hearts
  if (state.tab === 'pl' && !knownPl.has(state.pl)) {
    if (had.has(state.pl)) switchTab('songs', true); // deleted (maybe elsewhere)
    else if (ctx.loaded) render(); // first load: show "not found"
    return;
  }
  const id = state.tab === 'fav' ? FAV : state.tab === 'pl' ? state.pl : null;
  if (id == null || !ctx.loaded) return; // loadLibrary renders the page
  if (c.id == null || (id === FAV ? c.favorites : c.id === id)) {
    loadItems(id).then(() => { if (isPlTab(state.tab)) render(); }, showErr);
  }
}
document.querySelectorAll('#nav,#mtabs,#favnav').forEach((el) => el.addEventListener('click', (e) => { const b = e.target.closest('[data-tab]'); if (b) switchTab(b.dataset.tab); }));
function resetSearch() { $('q').value = ''; state.q = ''; }
let qt;
$('q').addEventListener('input', (e) => { clearTimeout(qt); qt = setTimeout(() => { state.q = e.target.value; state.open = null; render(); }, 120); });
$('clearq').onclick = () => { resetSearch(); render(); $('q').focus(); };
document.addEventListener('keydown', (e) => {
  const typing = /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName) || $('settings').open || $('namedlg').open || $('confirmdlg').open;
  if (e.key === '/' && !typing) { e.preventDefault(); $('q').focus(); }
  if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'a' && !typing && ctx.view && ctx.list.length) {
    e.preventDefault();
    selectAll(ctx.sel, listIds());
    paintSel();
  }
  if (e.key === 'Escape') {
    closeDrawer();
    closeMenus();
    if (!typing) clearSel();
  }
});
window.addEventListener('scroll', () => $('topbar').classList.toggle('scrolled', scrollY > 4), { passive: true });
// Row height changes at the narrow breakpoint.
matchMedia('(max-width:760px)').addEventListener('change', () => { if (ctx.view) ctx.view.refresh(); });

// ---------- top bar actions (the refresh button is wired in settings.js) ----------
$('opensettings').onclick = () => ctx.openSettings();

function fromHash() {
  const h = location.hash.slice(1), m = /^pl\/(\d+)$/.exec(h);
  if (m) return { tab: 'pl', pl: +m[1] };
  return { tab: ['songs', 'artists', 'albums', 'fav'].includes(h) ? h : 'songs', pl: null };
}

// ---------- boot ----------
async function boot() {
  const qs = new URLSearchParams(location.search);
  if (isMock && ['light', 'dark'].includes(qs.get('theme'))) document.documentElement.dataset.theme = qs.get('theme');
  await initI18n(isMock ? qs.get('lang') || undefined : undefined);
  applyI18n();
  state.shelfSort = store.get('shelfSort', 'count');
  const h = fromHash();
  state.tab = h.tab; state.pl = h.pl;
  state.plSort = isPlTab(state.tab) ? defaultPlSort(state.tab) : null;
  initSettings(ctx, { loadLibrary });
  const plReady = initPlaylists(ctx);
  try {
    await loadLibrary();
    await plReady;
  } catch (e) {
    $('view').innerHTML = `<div class="empty">${icon('disc')}<h2>${esc(t('error.loadTitle'))}</h2><p>${esc(String(e && e.message || e))}</p></div>`;
    return;
  }
  await initPlayer(ctx);
  // first start: nothing to show until a folder is added
  if (!ctx.lib.roots.length) ctx.openSettings();
}

export const ready = boot();
export { render, loadLibrary, sidebar, rowHeight };
