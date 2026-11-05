import { t } from './i18n.js';

export const esc = (x) => String(x).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);

export const mmss = (s) => {
  s = Math.max(0, Math.floor(s || 0));
  return s >= 3600
    ? Math.floor(s / 3600) + ':' + String(Math.floor((s % 3600) / 60)).padStart(2, '0') + ':' + String(s % 60).padStart(2, '0')
    : Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
};

export const hm = (s) => {
  const h = Math.floor(s / 3600), m = Math.round((s % 3600) / 60);
  return h ? t('time.hm', { h, m }) : t('time.m', { m });
};

export const norm = (x) => (x || '').normalize('NFKC').toLowerCase();

const LOSSLESS = ['pcm', 'flac', 'alac', 'ape'];
const LABELS = { pcm: 'WAV', flac: 'FLAC', alac: 'ALAC', mp3: 'MP3', wma: 'WMA', ape: 'APE', ogg: 'OGG', opus: 'OPUS' };

/** Format by the real codec: some files carry the wrong extension (a WAV renamed to .mp3). */
export function fmtLabel(s) {
  return LABELS[s.c] || (s.c === 'aac' && s.f !== 'aac' ? 'M4A' : String(s.f || '').toUpperCase());
}

export function quality(s) {
  return LOSSLESS.includes(s.c) ? 'lossless' : s.br >= 250 ? 'high' : s.br >= 160 ? 'std' : 'low';
}

/** Codecs the built-in decoder lacks (symphonia 0.5 has no WMA, APE or Opus decoder). */
const NEED_FFMPEG = ['wma', 'ape', 'opus'];

/** Needs FFmpeg for WMA/APE/Opus; songs in an unreachable folder are never playable. */
export function playable(s, caps) {
  return !!s.av && (!!(caps && caps.ffmpeg) || !NEED_FFMPEG.includes(s.c));
}

/**
 * Sets each song's `av` from its folder's online flag. Returns true if any song changed.
 * Songs of folders not in `roots` are left as they are.
 */
export function applyOnline(songs, roots) {
  const on = new Map(roots.map((r) => [r.id, !!r.online]));
  let changed = false;
  for (const s of songs) {
    if (!on.has(s.r)) continue;
    const av = on.get(s.r);
    if (s.av !== av) { s.av = av; changed = true; }
  }
  return changed;
}

/** Events that end a slider drag. `change` alone is not enough: a drag that returns to its start value fires none. */
export const DRAG_END = ['change', 'pointerup', 'pointercancel', 'blur', 'keyup'];

/**
 * Range-input drag that cannot get stuck. `input` starts it (and calls `preview(v)`);
 * any DRAG_END event ends it once, calling `end(v)` with the final value if it differs from
 * `current()` (the value already in effect), else `end(null)`. `dragging()` tells whether
 * a drag is in progress (to ignore outside updates meanwhile).
 */
export function trackSlider(el, { preview, end, current }) {
  let active = false;
  el.addEventListener('input', () => { active = true; preview(+el.value); });
  const finish = () => {
    if (!active) return;
    active = false;
    const v = +el.value;
    end(v !== current() ? v : null);
  };
  for (const ev of DRAG_END) el.addEventListener(ev, finish);
  return { dragging: () => active };
}

let fallbackCover = null;
/** Dev preview only: `fn(song) -> url` used when there is no Tauri runtime. */
export function setCoverSource(fn) {
  fallbackCover = fn;
}

export function coverUrl(s) {
  if (!s || !s.pc) return null;
  const core = globalThis.__TAURI__ && globalThis.__TAURI__.core;
  if (core && core.convertFileSrc) return core.convertFileSrc(s.id, 'cover') + `?v=${s.z}-${s.pc}`;
  return fallbackCover ? fallbackCover(s) : null;
}

export const icon = (id, cls = '') => `<svg class="i ${cls}"><use href="#i-${id}"/></svg>`;
