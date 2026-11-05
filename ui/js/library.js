// Pure library queries (no DOM). state: { q, artist, album, quality, root, sort:{col,dir}, shelfSort }.
// artist '' means "unknown artist"; null means no filter.
import { norm, quality } from './util.js';

const LAST = '￿';
const cache = new WeakMap();

/** Derived, normalized fields for a song, computed once per song object. */
export function info(s) {
  let x = cache.get(s);
  if (!x) {
    x = {
      q: quality(s),
      key: norm([s.t, s.a, s.b, s.p].join(' ')),
      t: norm(s.t),
      a: s.a ? norm(s.a) : LAST,
      b: s.b ? norm(s.b) : LAST,
    };
    cache.set(s, x);
  }
  return x;
}

function filterSongs(songs, state, withPeople) {
  const q = norm((state.q || '').trim());
  return songs.filter((s) => {
    const x = info(s);
    return (!q || x.key.includes(q)) &&
      (!withPeople || state.artist == null || s.a === state.artist) &&
      (!withPeople || state.album == null || s.b === state.album) &&
      (state.root == null || s.r === state.root) &&
      (!state.quality || x.q === state.quality);
  });
}

function sortValue(s, col) {
  const x = info(s);
  switch (col) {
    case 'n': return s.n || 999;
    case 's': return s.s;
    case 'br': return x.q === 'lossless' ? 9999 : s.br;
    case 'a': return x.a;
    case 'b': return x.b;
    default: return x.t;
  }
}

/** Search / artist / album / quality / folder filters, then the web version's sort. */
export function selectSongs(songs, state) {
  const { col, dir } = state.sort || { col: 'a', dir: 1 };
  const list = filterSongs(songs, state, true).map((s) => ({ s, v: sortValue(s, col) }));
  list.sort(({ s: x, v: a }, { s: y, v: b }) => {
    if (a < b) return -dir;
    if (a > b) return dir;
    if (x.a !== y.a) return x.a < y.a ? -1 : 1;
    if (x.b !== y.b) return x.b < y.b ? -1 : 1;
    return (x.n || 0) - (y.n || 0) || (x.t < y.t ? -1 : x.t > y.t ? 1 : 0);
  });
  return list.map((e) => e.s);
}

/** Artist or album groups: [{ name, songs }]. Unknown artist is name ''; songs without album are left out. */
export function groupShelf(songs, kind, state) {
  const q = norm((state.q || '').trim());
  const groups = new Map();
  for (const s of filterSongs(songs, { ...state, q: '' }, false)) {
    const k = kind === 'artists' ? s.a || '' : s.b;
    if (kind === 'albums' && !k) continue;
    let g = groups.get(k);
    if (!g) groups.set(k, (g = { name: k, songs: [] }));
    g.songs.push(s);
  }
  const list = [...groups.values()].filter((g) => !q || norm(g.name).includes(q) || g.songs.some((s) => info(s).key.includes(q)));
  const byName = (a, b) => a.name.localeCompare(b.name, 'zh');
  list.sort(state.shelfSort === 'name' ? byName : (a, b) => b.songs.length - a.songs.length || byName(a, b));
  return list;
}
