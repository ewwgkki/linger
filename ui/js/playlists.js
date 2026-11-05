// Playlists and favorites: cached backend data, the `playlists://changed` event, and pure helpers.
// Favorites ("liked songs") are the special playlist id 0 here (save_songs uses the same id).
import { api } from './api.js';
import { norm } from './util.js';
import { info, selectSongs } from './library.js';

export const FAV = 0;

export const pl = {
  /** PlaylistDto[] in sidebar order. */
  list: [],
  /** Ids of songs in favorites or any playlist. */
  saved: new Set(),
  /** Favorites, newest first (ItemDto[]). */
  favItems: [],
  /** True once the first fetch has returned. */
  loaded: false,
};

/** id -> ItemDto[] (resolved) and id -> pending fetch. */
const items = new Map();
const pending = new Map();
/** Bumped when the global epoch or an id is invalidated, so a fetch that started before is not cached. */
let epoch = 0;
const gens = new Map();
const stamp = (id) => epoch + ':' + (gens.get(id) || 0);
let ctx = null;

/**
 * Library rows for playlist items, in item order. Songs still in the library are the library object
 * plus `_i` (item id) and `_ad` (added at); songs no longer there are built from
 * the item's snapshot and marked `missing`.
 */
export function itemsToRows(list, songsById) {
  return list.map((it) => {
    const s = songsById.get(it.id);
    // prototype-linked rather than copied, so later changes to the library object (e.g. `av` when a
    // folder goes offline) show through
    if (s) return Object.assign(Object.create(s), { _i: it.i, _ad: it.ad });
    return { id: it.id, t: it.t, a: it.a, b: it.b, s: it.s, missing: true, _i: it.i, _ad: it.ad };
  });
}

/** Search filter and sort for a playlist page. `pos` keeps the playlist order, `ad` sorts by date added. */
export function selectRows(rows, q, sort) {
  const { col, dir } = sort || { col: 'pos', dir: 1 };
  if (col !== 'pos' && col !== 'ad') return selectSongs(rows, { q, sort: { col, dir } });
  const n = norm((q || '').trim());
  const list = n ? rows.filter((s) => info(s).key.includes(n)) : rows.slice();
  if (col === 'ad') {
    const at = new Map(list.map((s, i) => [s, i]));
    list.sort((a, b) => ((a._ad || 0) - (b._ad || 0)) * dir || at.get(a) - at.get(b));
  } else if (dir < 0) list.reverse();
  return list;
}

/** Toast text for a save_songs result as [i18n key, params]. Duplicates alone count as "nothing changed". */
export function saveResultKey({ added = 0, removed = 0 }) {
  if (added && removed) return ['save.changed', { n: added, r: removed }];
  if (added) return ['save.added', { n: added }];
  if (removed) return ['save.removed', { n: removed }];
  return ['save.nothing', {}];
}

export const isSaved = (id) => pl.saved.has(id);

/** Items already fetched for `id`, or null. */
export const cachedItems = (id) => items.get(id) || null;

/** Items of playlist `id` (FAV for favorites), cached until a change event for it. */
export function loadItems(id) {
  if (items.has(id)) return Promise.resolve(items.get(id));
  if (pending.has(id)) return pending.get(id);
  const g = stamp(id);
  const p = api.invoke(id === FAV ? 'get_favorites' : 'get_playlist', id === FAV ? undefined : { id })
    .then((list) => {
      if (g === stamp(id)) {
        items.set(id, list);
        if (id === FAV) pl.favItems = list;
      }
      return list;
    })
    .finally(() => { if (pending.get(id) === p) pending.delete(id); });
  pending.set(id, p);
  return p;
}

function invalidate(id) {
  if (id == null) { epoch++; items.clear(); pending.clear(); } else { gens.set(id, (gens.get(id) || 0) + 1); items.delete(id); pending.delete(id); }
}

async function fetchAll(withFav) {
  const [list, saved] = await Promise.all([api.invoke('get_playlists'), api.invoke('get_saved')]);
  pl.list = list || [];
  pl.saved = new Set(saved || []);
  if (withFav) await loadItems(FAV);
  pl.loaded = true;
}

let queue = Promise.resolve();
/** Re-fetches after a change; `ctx.playlistsChanged(change)` then updates the UI. Changes are handled in order. */
function onChanged(change) {
  const c = { id: change && change.id != null ? change.id : null, favorites: !!(change && change.favorites) };
  if (c.id == null) invalidate(null);
  else invalidate(c.id);
  if (c.favorites) invalidate(FAV);
  queue = queue.then(() => fetchAll(c.favorites || c.id == null))
    .then(() => ctx && ctx.playlistsChanged && ctx.playlistsChanged(c))
    .catch((e) => console.error('playlists', e));
  return queue;
}

/** First load, then follow `playlists://changed`. */
export async function initPlaylists(c) {
  ctx = c;
  api.listen('playlists://changed', onChanged);
  return onChanged({ id: null, favorites: true });
}

/** → PlaylistDto. Rejects with the backend's error (an i18n key). */
export async function createPlaylist(name, songIds = []) {
  const dto = await api.invoke('create_playlist', { name, songIds });
  // show it right away; the change event brings the backend's list shortly
  if (!pl.list.some((p) => p.id === dto.id)) pl.list = [...pl.list, dto];
  return dto;
}
export const renamePlaylist = (id, name) => api.invoke('rename_playlist', { id, name });
export const deletePlaylist = (id) => api.invoke('delete_playlist', { id });

/** Text of a backend error: its translation when it is an i18n key, else the raw message. */
export const errKey = (e) => String((e && e.message) || e);
