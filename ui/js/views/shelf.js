// Artist / album card grids.
import { t } from '../i18n.js';
import { esc, mmss, norm, icon } from '../util.js';
import { groupShelf } from '../library.js';
import { art, hl, chips, emptyLibrary, withCover, artistName } from './songs.js';

export function renderShelf(el, ctx, kind) {
  const { state, lib } = ctx;
  if (!lib.songs.length) {
    el.innerHTML = emptyLibrary();
    return;
  }
  const q = norm(state.q.trim());
  const list = groupShelf(lib.songs, kind, state);
  const artists = kind === 'artists';
  const hero = `<section class="hero"><div class="hero-txt"><span class="kicker">${esc(t('shelf.kicker'))}</span><h1>${esc(t(artists ? 'nav.artists' : 'nav.albums'))}</h1>` +
    `<div class="meta">${esc(t(artists ? 'meta.artistsCount' : 'meta.albumsCount', { n: list.length }))}</div></div></section>
    <div class="toolbar">${chips(ctx) || '<span></span>'}<div class="seg" id="shelfsort">
      <button data-v="count" aria-pressed="${state.shelfSort !== 'name'}">${esc(t('shelf.byCount'))}</button><button data-v="name" aria-pressed="${state.shelfSort === 'name'}">${esc(t('shelf.byName'))}</button></div></div>`;
  if (!list.length) {
    el.innerHTML = hero + `<div class="empty">${icon('search')}<p>${esc(t(artists ? 'shelf.noArtists' : 'shelf.noAlbums'))}</p></div>`;
    return;
  }
  const cards = list.map((g) => {
    const secs = g.songs.reduce((a, s) => a + s.s, 0);
    if (artists) {
      const albums = new Set(g.songs.map((s) => s.b).filter(Boolean)).size;
      const name = g.name ? hl(g.name, q) : `<span class="unk">${esc(artistName(''))}</span>`;
      return `<button class="card artist" type="button" data-artist="${esc(g.name)}">${art(withCover(g.songs), 'mic', 'md round')}
        <span class="name">${name}</span><span class="meta">${esc(t('meta.songs', { n: g.songs.length }))}${albums ? ' · ' + esc(t('meta.albums', { n: albums })) : ''}</span></button>`;
    }
    const names = [...new Set(g.songs.map((s) => s.a).filter(Boolean))];
    const by = names.slice(0, 2).join(t('list.sep')) + (names.length > 2 ? t('list.etc') : '');
    return `<button class="card" type="button" data-album="${esc(g.name)}">${art(withCover(g.songs), 'disc', 'md')}
      <span class="name">${hl(g.name, q)}</span><span class="meta">${esc(by) || '&nbsp;'}</span>
      <span class="meta">${esc(t('meta.songs', { n: g.songs.length }))} · ${mmss(secs)}</span></button>`;
  }).join('');
  el.innerHTML = `${hero}<div class="grid">${cards}</div>`;
}
