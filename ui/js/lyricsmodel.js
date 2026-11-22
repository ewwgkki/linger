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
