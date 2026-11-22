// Page and filter rules. A sidebar click enters a new page and clears everything; a folder,
// artist, album or list is a fixed page that can be narrowed by its own quality filter.
import { info } from './library.js';

const QUALITY_KEYS = ['lossless', 'high', 'std', 'low'];
const CLEAR = { q: '', artist: null, album: null, quality: null, root: null, pageQ: null, open: null };

/** Clears every filter, then applies `patch` (the page being entered). */
export const enterPage = (state, patch = {}) => Object.assign(state, CLEAR, patch);

/** Pages that show their own quality filter. */
export const isScopedPage = (s) => s.tab === 'fav' || s.tab === 'pl' ||
  (s.tab === 'songs' && (s.root != null || s.artist != null || s.album != null));

export const byPageQuality = (list, k) => (k ? list.filter((s) => !s.missing && info(s).q === k) : list);

/** [{ k: null (all) | quality, n }]; empty when there is nothing to choose between. */
export function pageQualityOptions(list, active = null) {
  const n = Object.fromEntries(QUALITY_KEYS.map((k) => [k, 0]));
  for (const s of list) if (!s.missing) n[info(s).q]++;
  const present = QUALITY_KEYS.filter((k) => n[k] > 0 || k === active);
  if (present.length < 2 && !active) return [];
  return [{ k: null, n: list.length }, ...present.map((k) => ({ k, n: n[k] }))];
}
