// Song list view: hero, sort toolbar, virtual list of rows. Also shared view helpers.
import { t } from '../i18n.js';
import { esc, mmss, hm, norm, fmtLabel, playable, coverUrl, icon } from '../util.js';
import { selectSongs, info } from '../library.js';
import { mountVList } from '../vlist.js';

export const QUALITIES = [
  { k: 'lossless', color: '--q-lossless' },
  { k: 'high', color: '--q-high' },
  { k: 'std', color: '--q-std' },
  { k: 'low', color: '--q-low' },
];
export const qLabel = (k) => t('q.' + k);

/** Highlights the search match when normalization kept the length (as in the web version). */
export function hl(text, q) {
  if (!q) return esc(text);
  const n = norm(text), i = n.indexOf(q);
  if (i < 0 || n.length !== text.length) return esc(text);
  return esc(text.slice(0, i)) + '<mark>' + esc(text.slice(i, i + q.length)) + '</mark>' + esc(text.slice(i + q.length));
}

export const withCover = (list) => list.find((s) => s.pc);

/** Cover image, or a symbol icon when there is none (or it fails to load). */
export function art(s, kind = 'note', cls = '') {
  const u = coverUrl(s);
  return `<span class="art ${cls}" aria-hidden="true">${icon(kind)}${u ? `<img src="${esc(u)}" alt="" loading="lazy" decoding="async">` : ''}</span>`;
}

export function badges(s) {
  const q = info(s).q;
  const txt = q === 'lossless' ? t('q.badgeLossless') : s.br + 'k';
  return `<span class="badge fmt">${esc(fmtLabel(s))}</span><span class="badge" style="background:var(${QUALITIES.find((x) => x.k === q).color})" title="${esc(qLabel(q))}">${esc(txt)}</span>`;
}

export const artistName = (a) => a || t('song.unknownArtist');
export const rootLabel = (r) => (r ? r.path.split(/[\\/]/).filter(Boolean).pop() || r.path : '');

/** Absolute path of a song, using the root's separator style. */
export function fullPath(roots, s) {
  const r = roots.find((x) => x.id === s.r);
  if (!r) return s.p;
  const sep = r.path.includes('\\') ? '\\' : '/';
  const base = /[\\/]$/.test(r.path) ? r.path : r.path + sep;
  return base + s.p.split('/').join(sep);
}

export function chips(ctx) {
  const { state, lib } = ctx;
  const c = [];
  if (state.q.trim()) c.push(`<button class="chip" data-clear="q">${esc(t('chip.search', { q: state.q.trim() }))}<span>&times;</span></button>`);
  if (state.quality) c.push(`<button class="chip" data-clear="quality">${esc(qLabel(state.quality))}<span>&times;</span></button>`);
  if (state.root != null) {
    const r = lib.roots.find((x) => x.id === state.root);
    c.push(`<button class="chip" data-clear="root">${esc(t('chip.folder', { name: rootLabel(r) }))}<span>&times;</span></button>`);
  }
  return c.length ? `<div class="chips">${c.join('')}</div>` : '';
}

export function emptyLibrary() {
  return `<div class="empty">${icon('disc')}<h2>${esc(t('empty.libTitle'))}</h2><p>${esc(t('empty.libText'))}</p>` +
    `<button class="btn primary lg" type="button" data-open-settings>${icon('folder')}${esc(t('empty.addFolder'))}</button></div>`;
}

const joinNames = (names, max) => names.slice(0, max).join(t('list.sep')) + (names.length > max ? t('list.etc') : '');

const SORTS = [['a', 'sort.artist'], ['t', 'sort.title'], ['b', 'sort.album'], ['s', 'sort.duration'], ['br', 'sort.quality']];

/** Row height from CSS (72px, 64px on narrow windows). */
export const rowHeight = () => parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--row-h')) || 72;
const DETAIL_H = 56;

function rowHtml(ctx, s, q) {
  const { state, caps } = ctx;
  const ok = playable(s, caps);
  const artist = s.a ? `<button class="link" data-artist="${esc(s.a)}">${hl(s.a, q)}</button>` : `<span class="unk">${esc(t('song.unknownArtist'))}</span>`;
  const album = s.b && state.album == null
    ? `<button class="link" data-album="${esc(s.b)}">${t('song.album', { b: hl(s.b, q) })}</button><span class="sep">-</span>` : '';
  const why = !s.av ? t('song.offline') : !ok ? t('song.needFfmpeg', { fmt: fmtLabel(s) }) : '';
  const open = state.open === s.id;
  const cls = `song${s.id === ctx.nowId ? ' now' : ''}${open ? ' open' : ''}${s.av ? '' : ' off'}`;
  const play = ok
    ? `<button class="rbtn play" type="button" data-play="${esc(s.id)}" aria-label="${esc(t('song.playAria', { t: s.t }))}" title="${esc(t('song.play'))}">`
    : `<button class="rbtn play" type="button" disabled aria-label="${esc(why)}" title="${esc(why)}">`;
  const row = `<div class="${cls}" data-id="${esc(s.id)}" tabindex="0" role="button" aria-expanded="${open}"${why ? ` title="${esc(why)}"` : ''}>
    ${art(s)}
    <div class="s-main">
      <div class="s-l1"><span class="eq"><i></i><i></i><i></i></span>${s.n && state.album != null ? `<span class="tn">${String(s.n).padStart(2, '0')}</span>` : ''}<span class="t">${hl(s.t, q)}</span>${badges(s)}</div>
      <div class="s-l2">${album}${artist}</div>
    </div>
    <div class="s-right">
      <span class="dur">${mmss(s.s)}</span>
      ${play}<svg class="i f" viewBox="0 0 24 24"><circle cx="12" cy="12" r="11"/><path d="M10 8.2v7.6a.6.6 0 0 0 .9.5l6-3.8a.6.6 0 0 0 0-1l-6-3.8a.6.6 0 0 0-.9.5z" fill="var(--surface)"/></svg></button>
    </div></div>`;
  if (!open) return row;
  const det = `<div class="detail">
    <span class="path" id="path-${esc(s.id)}">${esc(fullPath(ctx.lib.roots, s))}</span>
    <span>${esc(t('detail.info', { mb: (s.z / 1048576).toFixed(1), fmt: fmtLabel(s), br: s.br, dur: mmss(s.s) }))}</span>
    ${s.b && state.album == null ? `<button class="btn" type="button" data-album="${esc(s.b)}">${icon('disc')}${esc(t('detail.viewAlbum'))}</button>` : ''}
    <button class="btn" type="button" data-copy="${esc(s.id)}">${esc(t('detail.copyPath'))}</button></div>`;
  return `<div class="vgrp">${row}${det}</div>`;
}

/** Renders the songs page into `el`. Returns { list, vlist, refresh() }. */
export function renderSongs(el, ctx) {
  const { state, lib, caps } = ctx;
  const q = norm(state.q.trim());
  const list = selectSongs(lib.songs, state);
  if (!lib.songs.length) {
    el.innerHTML = emptyLibrary();
    return { list, vlist: null, refresh() {} };
  }
  const secs = list.reduce((a, s) => a + s.s, 0);
  const playableCount = list.filter((s) => playable(s, caps)).length;
  const acts = `<div class="hero-acts">
      <button class="btn primary lg" type="button" id="playall" ${playableCount ? '' : 'disabled'}>${icon('play', 'f')}${esc(t('hero.play'))}</button>
      <button class="btn lg" type="button" id="shuffleall" ${playableCount ? '' : 'disabled'}>${icon('shuffle')}${esc(t('hero.shuffle'))}</button>
      ${playableCount < list.length ? `<span class="hint">${esc(t('hero.unplayable', { n: list.length - playableCount }))}</span>` : ''}
    </div>`;
  const songsMeta = `${esc(t('meta.songs', { n: list.length }))} · ${esc(hm(secs))}`;
  let hero;
  if (state.album != null) {
    const arts = [...new Set(list.map((s) => s.a).filter(Boolean))];
    const links = arts.slice(0, 3).map((a) => `<button data-artist="${esc(a)}">${esc(a)}</button>`).join(esc(t('list.sep'))) + (arts.length > 3 ? esc(t('list.etc')) : '');
    hero = `<section class="hero has-cv">${art(withCover(list), 'disc', 'lg')}<div class="hero-txt"><span class="kicker">${esc(t('hero.album'))}</span><h1>${esc(state.album)}</h1>
      <div class="meta">${links}${arts.length ? ' · ' : ''}${songsMeta}</div>${acts}</div></section>`;
  } else if (state.artist != null) {
    const albums = new Set(list.map((s) => s.b).filter(Boolean)).size;
    hero = `<section class="hero has-cv">${art(withCover(list), 'mic', 'lg round')}<div class="hero-txt"><span class="kicker">${esc(t('hero.artist'))}</span><h1>${esc(artistName(state.artist))}</h1>
      <div class="meta">${esc(t('meta.songs', { n: list.length }))}${albums ? ' · ' + esc(t('meta.albums', { n: albums })) : ''} · ${esc(hm(secs))}</div>${acts}</div></section>`;
  } else {
    const title = q ? t('hero.search') : state.quality ? qLabel(state.quality) : t('hero.allSongs');
    hero = `<section class="hero"><div class="hero-txt"><span class="kicker">${esc(t('hero.songs'))}</span><h1>${esc(title)}</h1>
      <div class="meta">${songsMeta}</div>${acts}</div></section>`;
  }
  const { col, dir } = state.sort;
  const sorts = state.album != null ? [['n', 'sort.track'], ...SORTS.filter((x) => x[0] !== 'b')] : SORTS;
  hero += `<div class="toolbar">${chips(ctx) || '<span></span>'}<div class="sorts"><span>${esc(t('sort.label'))}</span><div class="seg" id="songsort">${
    sorts.map(([c, k]) => `<button data-sort="${c}" aria-pressed="${col === c}" data-dir="${dir > 0 ? '↑' : '↓'}">${esc(t(k))}</button>`).join('')}</div></div></div>`;
  if (!list.length) {
    el.innerHTML = hero + `<div class="empty">${icon('search')}<p>${esc(t('empty.noMatch'))}<br>${esc(t('empty.noMatchHint'))}</p></div>`;
    return { list, vlist: null, refresh() {} };
  }
  el.innerHTML = hero + '<div class="songs" id="songlist"></div>';
  const expanded = () => {
    const i = state.open ? list.findIndex((s) => s.id === state.open) : -1;
    return i >= 0 ? { index: i, extra: DETAIL_H } : null;
  };
  const vlist = mountVList(el.querySelector('#songlist'), {
    count: list.length,
    rowH: rowHeight(),
    render: (i) => rowHtml(ctx, list[i], q),
    expanded: expanded(),
  });
  return { list, vlist, refresh: () => vlist.update({ rowH: rowHeight(), expanded: expanded() }) };
}
