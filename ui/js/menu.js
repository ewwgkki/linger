// Song context menu, and the song actions it shares with the selection bar.
import { t } from './i18n.js';
import { api } from './api.js';
import { esc, icon, playable } from './util.js';
import { FAV, saveResultKey, errKey } from './playlists.js';
import { saveSongs, place, closePicker } from './savepicker.js';
import { fullPath } from './views/songs.js';
import { toast } from './settings.js';
import { playerState, clearSource } from './player.js';

const $ = (id) => document.getElementById(id);
const showErr = (e) => toast(t(errKey(e)), { icon: 'x', autoHide: 6000 });
const done = (msg) => toast(msg, { icon: 'check', autoHide: 3000 });

/** Rows of the current list for `ids`, in list order. */
const rowsOf = (ids, ctx) => { const set = new Set(ids); return ctx.list.filter((s) => set.has(s.id)); };
const playableOf = (ids, ctx) => rowsOf(ids, ctx).filter((s) => !s.missing && playable(s, ctx.caps));

/** Plays the selected songs, in list order. */
export function playIds(ids, ctx) {
  const l = playableOf(ids, ctx);
  if (l.length) ctx.playList(l, 0, false);
}

/** Plays the current list from song `id` (like a double-click on its row). */
function playFrom(id, ctx) {
  const l = ctx.list.filter((s) => !s.missing && playable(s, ctx.caps));
  const i = l.findIndex((s) => s.id === id);
  if (i >= 0) ctx.playList(l, i, false);
}

/** Adds the songs to the user queue (`next`: right after the current song). */
export async function queueIds(ids, next, ctx) {
  const l = playableOf(ids, ctx).map((s) => s.id);
  if (!l.length) return;
  try {
    // with nothing playing the backend plays these songs as a new list with no source page
    if (!playerState().id) clearSource();
    await api.invoke('player_queue_add', { ids: l, next: !!next });
    done(t(next ? 'menu.queuedNext' : 'menu.queued', { n: l.length }));
  } catch (e) { showErr(e); }
}

/** Removes the songs from the open playlist page (by item id) or from favorites. */
export async function removeIds(ids, ctx, itemIds) {
  const { state } = ctx;
  try {
    if (state.tab === 'fav') {
      const r = await api.invoke('save_songs', { songIds: ids, add: [], remove: [FAV] });
      const [k, p] = saveResultKey(r || {});
      done(t(k, p));
    } else if (state.tab === 'pl') {
      const items = itemIds || rowsOf(ids, ctx).map((s) => s._i);
      await api.invoke('playlist_remove', { id: state.pl, itemIds: items });
      done(t('save.removed', { n: items.length }));
    }
  } catch (e) { showErr(e); }
}

/** Songs that can be saved (not missing). */
export const savableIds = (ids, ctx) => rowsOf(ids, ctx).filter((s) => !s.missing).map((s) => s.id);

function copyText(text) {
  if (navigator.clipboard && window.isSecureContext) return navigator.clipboard.writeText(text);
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.style.cssText = 'position:fixed;opacity:0;top:0;left:0';
  document.body.append(ta);
  ta.select();
  const ok = document.execCommand('copy');
  ta.remove();
  return ok ? Promise.resolve() : Promise.reject(new Error('copy'));
}

let open = null;
export const menuOpen = () => !!open;

export function closeMenus() {
  closePicker();
  if (!open) return;
  const { prevFocus } = open;
  open = null;
  $('menu').hidden = true;
  $('menu').innerHTML = '';
  document.removeEventListener('pointerdown', onOutside, true);
  document.removeEventListener('keydown', onKey, true);
  window.removeEventListener('resize', closeMenus);
  window.removeEventListener('scroll', closeMenus);
  if (prevFocus && prevFocus.isConnected) prevFocus.focus({ preventScroll: true });
}

function onOutside(e) { if (open && !$('menu').contains(e.target)) closeMenus(); }

function onKey(e) {
  if (!open) return;
  const items = [...$('menu').querySelectorAll('[role=menuitem]:not([disabled])')];
  const i = items.indexOf(document.activeElement);
  if (e.key === 'Escape') { e.preventDefault(); e.stopImmediatePropagation(); closeMenus(); return; }
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    items[(i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
  } else if (e.key === 'Home' || e.key === 'End') {
    e.preventDefault();
    items[e.key === 'Home' ? 0 : items.length - 1]?.focus();
  } else if (e.key === 'Tab') {
    e.preventDefault();
  }
}

/**
 * Context menu at window point (x, y) for songs `ids` (in list order) of the current list.
 * `itemIds`: their playlist item ids on a playlist page.
 */
export function openSongMenu(x, y, ids, ctx, { itemIds } = {}) {
  closeMenus();
  const rows = rowsOf(ids, ctx);
  if (!rows.length) return;
  const one = rows.length === 1 ? rows[0] : null;
  const live = rows.filter((s) => !s.missing);
  const canPlay = live.some((s) => playable(s, ctx.caps));
  const { tab } = ctx.state;
  const items = [];
  if (live.length) {
    items.push(['play', 'play', 'menu.play', !canPlay], ['next', 'next', 'menu.playNext', !canPlay], ['queue', 'list', 'menu.addQueue', !canPlay],
      ['save', 'heart', 'menu.saveTo', false]);
    if (one) {
      items.push(['-']);
      if (one.a) items.push(['artist', 'mic', 'menu.viewArtist', false]);
      if (one.b) items.push(['album', 'disc', 'menu.viewAlbum', false]);
      items.push(['copy', 'folder', 'menu.copyPath', false]);
    }
  }
  if (tab === 'pl' || tab === 'fav') {
    if (items.length) items.push(['-']);
    items.push(['remove', 'trash', tab === 'pl' ? 'menu.removeFromPl' : 'menu.removeFromFav', false]);
  }
  showMenu(x, y, items, (act, r) => {
    const list = rows.map((s) => s.id);
    if (act === 'play') { if (one) playFrom(one.id, ctx); else playIds(list, ctx); }
    else if (act === 'next' || act === 'queue') queueIds(list, act === 'next', ctx);
    else if (act === 'save') saveSongs(live.map((s) => s.id), { x: r.left, y: r.top }, ctx);
    else if (act === 'artist') ctx.goArtist(one.a);
    else if (act === 'album') ctx.goAlbum(one.b);
    else if (act === 'copy') copyText(fullPath(ctx.lib.roots, one)).then(() => done(t('menu.copied')), showErr);
    else if (act === 'remove') removeIds(list, ctx, itemIds);
  });
}

/**
 * Context menu for a row of the play queue panel: song `id`; `onJump` / `onRemove` are given for
 * rows that can be played / removed (not for the playing song).
 */
export function openQueueMenu(x, y, id, ctx, { onJump, onRemove } = {}) {
  closeMenus();
  const s = ctx.songsById.get(id);
  const items = [];
  if (onJump) items.push(['jump', 'play', 'queue.playThis', !(s && playable(s, ctx.caps))]);
  if (s) items.push(['save', 'heart', 'menu.saveTo', false]);
  if (onRemove) items.push(...(items.length ? [['-']] : []), ['remove', 'trash', 'queue.remove', false]);
  if (!items.length) return;
  showMenu(x, y, items, (act, r) => {
    if (act === 'jump') onJump();
    else if (act === 'save') saveSongs([id], { x: r.left, y: r.top }, ctx);
    else if (act === 'remove') onRemove();
  });
}

/** Renders `items` ([act, icon, label key, disabled] or ['-']) at (x, y); `run(act, menuRect)` after closing. */
function showMenu(x, y, items, run) {
  const el = $('menu');
  el.innerHTML = items.map(([k, ic, label, off]) => (k === '-' ? '<div class="m-sep" role="separator"></div>'
    : `<button type="button" role="menuitem" class="m-item${k === 'remove' ? ' danger' : ''}" data-act="${k}"${off ? ' disabled' : ''}>${icon(ic, ic === 'play' || ic === 'next' ? 'f' : '')}<span>${esc(t(label))}</span></button>`)).join('');
  el.hidden = false;
  place(el, { x, y });
  open = { prevFocus: document.activeElement };
  document.addEventListener('pointerdown', onOutside, true);
  document.addEventListener('keydown', onKey, true);
  window.addEventListener('resize', closeMenus);
  window.addEventListener('scroll', closeMenus, { passive: true });
  el.onclick = (e) => {
    const b = e.target.closest('[data-act]');
    if (!b || b.disabled) return;
    const r = el.getBoundingClientRect();
    closeMenus();
    run(b.dataset.act, r);
  };
  el.querySelector('[role=menuitem]:not([disabled])')?.focus({ preventScroll: true });
}
