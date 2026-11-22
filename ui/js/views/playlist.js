// Playlist page (state.tab 'pl', state.pl = id) and the liked-songs page (state.tab 'fav').
import { t } from '../i18n.js';
import { esc, hm, norm, playable, icon } from '../util.js';
import { api } from '../api.js';
import { mountVList } from '../vlist.js';
import { attachReorder } from '../reorder.js';
import { pl, FAV, cachedItems, loadItems, itemsToRows, selectRows, errKey, moveCached, refetchItems } from '../playlists.js';
import { art, withCover, rowHtml, rowHeight, SORTS, DETAIL_H, chips, pageQualityBar, toolbarLeft } from './songs.js';
import { byPageQuality } from '../filters.js';
import { toast } from '../settings.js';

/** Rows per item list, rebuilt when the list or the library changes. */
const rowsCache = new WeakMap();
function rowsFor(list, byId) {
  const c = rowsCache.get(list);
  if (c && c.byId === byId) return c.rows;
  const rows = itemsToRows(list, byId);
  rowsCache.set(list, { byId, rows });
  return rows;
}

/** Default sort when entering a page: playlist order, or newest first for favorites. */
export const defaultPlSort = (tab) => (tab === 'fav' ? { col: 'ad', dir: -1 } : { col: 'pos', dir: 1 });

const none = (list = []) => ({ list, vlist: null, refresh() {} });

const emptyBox = (ic, title, text, extra = '') =>
  `<div class="empty">${icon(ic)}${title ? `<h2>${esc(title)}</h2>` : ''}${text ? `<p>${esc(text)}</p>` : ''}${extra}</div>`;

/** Renders the page into `el`. Returns { list, vlist, refresh() }; re-renders itself once items arrive. */
export function renderPlaylist(el, ctx) {
  const { state, caps } = ctx;
  const fav = state.tab === 'fav';
  const id = fav ? FAV : state.pl;
  const meta = fav ? null : pl.list.find((p) => p.id === id);
  if (!fav && !meta) {
    el.innerHTML = pl.loaded
      ? emptyBox('list', t('playlist.notFound'), '', `<button class="btn" type="button" data-tab-go="songs">${esc(t('playlist.backToSongs'))}</button>`)
      : emptyBox('list', '', t('playlist.loading'));
    return none();
  }
  const items = cachedItems(id);
  if (!items) {
    el.innerHTML = emptyBox(fav ? 'heart' : 'list', '', t('playlist.loading'));
    loadItems(id).then(() => {
      if (state.tab === (fav ? 'fav' : 'pl') && (fav || state.pl === id)) ctx.render();
    }, (e) => {
      if (state.tab === (fav ? 'fav' : 'pl') && (fav || state.pl === id)) el.innerHTML = emptyBox('x', t(errKey(e)), '');
    });
    return none();
  }

  const q = norm(state.q.trim());
  const all = rowsFor(items, ctx.songsById);
  const matched = selectRows(all, state.q, state.plSort || defaultPlSort(state.tab));
  const list = byPageQuality(matched, state.pageQ);
  const secs = list.reduce((a, s) => a + (s.s || 0), 0);
  const playableCount = list.filter((s) => playable(s, caps)).length;
  const lost = list.filter((s) => s.missing).length, blocked = list.length - playableCount - lost;
  const title = fav ? t('fav.title') : meta.name;
  const edit = fav ? '' : `
      <button class="btn lg" type="button" data-pl-rename="${id}">${icon('edit')}${esc(t('playlist.rename'))}</button>
      <button class="btn lg" type="button" data-pl-delete="${id}">${icon('trash')}${esc(t('playlist.delete'))}</button>`;
  const acts = `<div class="hero-acts">
      <button class="btn primary lg" type="button" id="playall" ${playableCount ? '' : 'disabled'}>${icon('play', 'f')}${esc(t('hero.play'))}</button>
      <button class="btn lg" type="button" id="shuffleall" ${playableCount ? '' : 'disabled'}>${icon('shuffle')}${esc(t('hero.shuffle'))}</button>${edit}
      ${blocked ? `<span class="hint">${esc(t('hero.unplayable', { n: blocked }))}</span>` : ''}
      ${lost ? `<span class="hint">${esc(t('playlist.missingCount', { n: lost }))}</span>` : ''}
    </div>`;
  let html = `<section class="hero has-cv">${art(withCover(all), fav ? 'heart' : 'list', 'lg')}<div class="hero-txt">
      <span class="kicker">${esc(t(fav ? 'fav.kicker' : 'playlist.kicker'))}</span><h1>${esc(title)}</h1>
      <div class="meta">${esc(t('meta.songs', { n: list.length }))} · ${esc(hm(secs))}</div>${acts}</div></section>`;

  if (!all.length) {
    el.innerHTML = html + emptyBox(fav ? 'heart' : 'list', t(fav ? 'fav.emptyTitle' : 'playlist.emptyTitle'), t(fav ? 'fav.emptyText' : 'playlist.emptyText'));
    return none(list);
  }
  const { col, dir } = state.plSort || defaultPlSort(state.tab);
  const sorts = [fav ? ['ad', 'sort.added'] : ['pos', 'sort.custom'], ...SORTS];
  html += `<div class="toolbar">${toolbarLeft(pageQualityBar(matched, state.pageQ), chips(ctx))}<div class="sorts"><span>${esc(t('sort.label'))}</span><div class="seg" id="plsort">${
    sorts.map(([c, k]) => `<button data-psort="${c}" aria-pressed="${col === c}" data-dir="${dir > 0 ? '↑' : '↓'}">${esc(t(k))}</button>`).join('')}</div></div></div>`;
  if (!list.length) {
    el.innerHTML = html + emptyBox('search', '', t('empty.noMatch'));
    return none(list);
  }
  // drag to reorder: playlists in custom order (either direction), unfiltered
  const grip = !fav && col === 'pos' && !q && !state.pageQ && list.length > 1;
  el.innerHTML = html + `<div class="songs${grip ? ' reorderable' : ''}" id="songlist"></div>`;
  const expanded = () => {
    const i = state.open ? list.findIndex((s) => s.id === state.open && !s.missing) : -1;
    return i >= 0 ? { index: i, extra: DETAIL_H } : null;
  };
  const box = el.querySelector('#songlist');
  const vlist = mountVList(box, {
    count: list.length,
    rowH: rowHeight(),
    render: (i) => rowHtml(ctx, list[i], q, grip),
    expanded: expanded(),
  });
  const detach = grip ? attachReorder(box, {
    rowH: rowHeight,
    handle: '[data-grip]',
    count: () => list.length,
    scroller: window,
    bounds: visibleBand,
    onStart() {
      if (!state.open) return;
      state.open = null;
      vlist.update({ expanded: null });
    },
    onMove(from, to) {
      const n = list.length, real = (i) => (dir < 0 ? n - 1 - i : i);
      const row = list[from];
      const refocus = box.contains(document.activeElement);
      // optimistic: `list` is also ctx.list
      list.splice(to, 0, ...list.splice(from, 1));
      moveCached(id, row._i, real(to));
      vlist.update({ expanded: expanded() });
      if (refocus) vlist.node(to)?.querySelector('[data-grip]')?.focus({ preventScroll: true });
      api.invoke('playlist_move', { id, itemId: row._i, to: real(to) }).catch((e) => {
        toast(t(errKey(e)), { icon: 'x', autoHide: 6000 });
        refetchItems(id).then(() => { if (state.tab === 'pl' && state.pl === id) ctx.render(); }, () => {});
      });
    },
  }) : null;
  return {
    list,
    vlist,
    refresh: () => vlist.update({ rowH: rowHeight(), expanded: expanded() }),
    destroy() { if (detach) detach(); vlist.destroy(); },
  };
}

/** Part of the window between the top bar and the player bar (drag auto-scroll edges). */
function visibleBand() {
  const bar = document.querySelector('.topbar'), p = document.getElementById('player');
  return { top: bar ? bar.getBoundingClientRect().bottom : 0, bottom: innerHeight - (p && !p.hidden ? p.offsetHeight : 0) };
}
