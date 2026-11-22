// Pure lyric timing logic (no DOM, no window): current line, highlight progress, playback clock.

const clamp01 = (x) => (x < 0 ? 0 : x > 1 ? 1 : x);

/** Index of the current line (last one starting at or before `ms`); -1 before the first, or when untimed. */
export function lineIndex(lines, ms) {
  if (!lines || !lines.length || lines[0].start == null) return -1;
  let lo = 0, hi = lines.length - 1, r = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const s = lines[mid].start;
    if (s != null && s <= ms) { r = mid; lo = mid + 1; } else hi = mid - 1;
  }
  return r;
}

/** 0..1 fill of one word. */
export function wordFill(word, ms) {
  const d = word.end - word.start;
  if (!(d > 0)) return ms >= word.start ? 1 : 0;
  return clamp01((ms - word.start) / d);
}

/** 0..1 fill of a whole line: by word durations when it has words, else by its own span. */
export function wordProgress(line, ms) {
  const ws = line.words || [];
  if (ws.length) {
    let total = 0, done = 0;
    for (const w of ws) {
      const d = Math.max(0, w.end - w.start);
      total += d;
      done += d * wordFill(w, ms);
    }
    return total > 0 ? clamp01(done / total) : ms >= ws[0].start ? 1 : 0;
  }
  if (line.start == null) return 0;
  if (line.end == null || line.end <= line.start) return ms >= line.start ? 1 : 0;
  return clamp01((ms - line.start) / (line.end - line.start));
}

/** A line without a usable end fills until the next line; the fill never takes longer than this. */
const MAX_FILL_MS = 10000;
const LAST_FILL_MS = 5000;

/** 0..1 fill of line `i`: like wordProgress, but a line-timed line always gets a sensible span. */
export function lineFill(lines, i, ms) {
  const line = lines[i];
  if ((line.words || []).length || line.start == null) return wordProgress(line, ms);
  const next = lines[i + 1];
  const end = line.end > line.start ? line.end : next && next.start > line.start ? next.start : line.start + LAST_FILL_MS;
  return wordProgress({ ...line, end: Math.min(end, line.start + MAX_FILL_MS) }, ms);
}

/** Playback position (ms) that shows `line` as current; null for an untimed line. */
export function seekTarget(line, offset) {
  if (!line || line.start == null) return null;
  return Math.max(0, line.start - (offset || 0));
}

/** A file picked for song `idBefore` may be used only if that song is still playing and its dialog still open. */
export function pickStillValid(idBefore, idNow, dialogOpen) {
  return !!idBefore && idBefore === idNow && !!dialogOpen;
}

/** Auto-scroll gate: after `touch(now)` (the user scrolled) `allowed` is false for `ms`. */
export function scrollGate(ms) {
  let until = -Infinity;
  return {
    touch(now) { until = now + ms; },
    clear() { until = -Infinity; },
    allowed: (now) => now >= until,
  };
}

/**
 * Playback clock in ms. `set(pos, playing, now)` takes a report from the player; `at(now)` reads the
 * interpolated position. A report slightly behind what was already shown (< 400 ms) is ignored while
 * playing so the highlight never steps back; a bigger jump (a seek) is followed at once.
 */
export function clock() {
  let base = 0, t0 = 0, playing = false;
  const at = (now) => (playing ? base + Math.max(0, now - t0) : base);
  return {
    set(pos, isPlaying, now) {
      const cur = at(now);
      if (playing && isPlaying && pos < cur && cur - pos < 400) return;
      base = pos; t0 = now; playing = !!isPlaying;
    },
    /** A real jump (seek, new song): always snaps, even when only slightly backwards. */
    seek(pos, isPlaying, now) { base = pos; t0 = now; playing = !!isPlaying; },
    at,
  };
}

/** Marks a result that was superseded by a later call. */
export const STALE = Symbol('stale');

/** `const run = latest(); run(asyncFn)` calls asyncFn now; only the most recent run resolves with its value, older ones with STALE. */
export function latest() {
  let n = 0;
  return async (fn) => {
    const me = ++n;
    const r = await fn();
    return me === n ? r : STALE;
  };
}

const DEFAULT_BACKDROP = [58, 60, 74];
/** Brightest the backdrop's top colour may get; light ink stays well above 4.5:1 on it. */
const BACKDROP_MAX_LUM = 0.075;
const BACKDROP_INK = [251, 250, 247];
/** Secondary text must clear 4.5:1 after rounding to whole colour values. */
const TEXT_MIN_CONTRAST = 4.6;

const channelLum = (v) => { const x = v / 255; return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; };
const relLum = ([r, g, b]) => 0.2126 * channelLum(r) + 0.7152 * channelLum(g) + 0.0722 * channelLum(b);
const ratio = (a, b) => { const [x, y] = [relLum(a), relLum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
const hex = (rgb) => '#' + rgb.map((v) => Math.round(v).toString(16).padStart(2, '0')).join('');

/**
 * Full-screen page colours from a cover colour: a darkened gradient that keeps the hue, with light
 * ink. `ink` is sung/current text, `mute` other lines and tools, `off` the unsung part of the current
 * line. Any input (also none) gives each of them at least 4.5:1 on both `top` and `bottom`.
 */
export function backdropColors(rgb) {
  const ok = Array.isArray(rgb) && rgb.length === 3 && rgb.every((v) => Number.isFinite(v));
  const base = (ok ? rgb : DEFAULT_BACKDROP).map((v) => Math.max(0, Math.min(255, v)));
  // pure black would give a flat page: lift it slightly
  const lifted = base.map((v) => v + (255 - v) * 0.07);
  let lo = 0, hi = 1;
  for (let i = 0; i < 16; i++) {
    const k = (lo + hi) / 2;
    if (relLum(lifted.map((v) => v * k)) <= BACKDROP_MAX_LUM) lo = k; else hi = k;
  }
  const top = lifted.map((v) => v * lo);
  const bottom = top.map((v) => v * 0.42);
  // the least ink (from `share` up) that stays readable on both ends of the gradient
  const level = (share) => {
    for (let k = share; k < 1; k += 0.01) {
      const c = BACKDROP_INK.map((v, i) => Math.round(v * k + top[i] * (1 - k)));
      if (ratio(c, top) >= TEXT_MIN_CONTRAST && ratio(c, bottom) >= TEXT_MIN_CONTRAST) return c;
    }
    return BACKDROP_INK;
  };
  return { top: hex(top), bottom: hex(bottom), ink: hex(BACKDROP_INK), mute: hex(level(0.74)), off: hex(level(0.6)) };
}
