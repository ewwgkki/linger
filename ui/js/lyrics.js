// Lyrics store: data for the playing song plus the playback clock. No DOM and no
// requestAnimationFrame here; the lyrics views (panel, full screen) run their own frame loop only
// while open and read `nowMs()` from it. Wired from player.js (onSong / onPos) and app.js (initLyrics).
import { api } from './api.js';
import { clock, latest, STALE } from './lyricsmodel.js';

/** id: playing song; data: last `lyrics_get` result (null until loaded); loading: request in flight. */
export const ly = { id: null, data: null, loading: false, trans: true, netease: true };

const RETRY_MS = 30000;
const MAX_OFFSET = 30000;
/** backend; tests swap it with a stub */
let be = api;
export const useBackend = (b) => { be = b; };
const subs = new Set();
const run = latest();
const clk = clock();
const perf = () => (globalThis.performance ? performance.now() : Date.now());
let active = false;
let retryTimer = 0;
let retried = false;
/** song id whose data was requested (so setActive does not refetch) */
let requested = null;
/** last offset the backend confirmed for the current data; offSeq numbers set-offset calls */
let confirmed = 0, confirmedSeq = 0, offSeq = 0;

const emit = () => { for (const fn of [...subs]) { try { fn(ly); } catch (e) { console.error('lyrics subscriber', e); } } };

/** Calls `fn(ly)` on every data change; returns an unsubscribe function. */
export function subscribe(fn) {
  subs.add(fn);
  return () => subs.delete(fn);
}

export function initLyrics() {
  return be.invoke('lyrics_settings').then((s) => {
    if (s) { ly.netease = !!s.netease; ly.trans = !!s.trans; emit(); }
  }).catch((e) => console.error('lyrics_settings', e));
}

async function load(id) {
  requested = id;
  ly.loading = true;
  emit();
  let data = null;
  try {
    data = await run(() => be.invoke('lyrics_get', { id }));
  } catch (e) {
    console.error('lyrics_get', e);
    if (ly.id === id) requested = null; // closing and reopening a view retries
  }
  if (data === STALE || ly.id !== id) return; // a later request or song change owns the state now
  ly.loading = false;
  ly.data = data;
  confirmed = (data && data.offset) || 0; confirmedSeq = offSeq;
  emit();
  if (data && data.status === 'retry' && !retried) {
    retried = true;
    clearTimeout(retryTimer);
    retryTimer = setTimeout(() => {
      if (ly.id !== id) return;
      if (active) load(id); else requested = null; // loaded when a view opens
    }, RETRY_MS);
  }
}

/** Playing song changed (null: nothing playing). Clears old data; fetches only while a view is open. */
export function onSong(id) {
  id = id || null;
  if (id === ly.id) return;
  clearTimeout(retryTimer);
  retried = false;
  requested = null;
  ly.id = id;
  ly.data = null;
  ly.loading = false;
  run(async () => null); // invalidate any request still in flight
  clk.seek(0, false, perf());
  confirmed = 0; confirmedSeq = offSeq = 0;
  emit();
  if (id && active) load(id);
}

/** Whether a lyrics view is open; lyrics are only fetched while one is. */
export function setActive(on) {
  active = !!on;
  if (active && ly.id && requested !== ly.id) load(ly.id);
}

/** Player position in seconds (as the player reports it) and whether it is playing. */
export function onPos(pos, playing) {
  clk.set(pos * 1000, !!playing, perf());
}

/** The user jumped (seek): snap the clock even if the jump is a few ms backwards. */
export function seekTo(pos, playing) {
  clk.seek(pos * 1000, !!playing, perf());
}

/** Playback position in ms including the song's lyric offset (positive offset = lyrics earlier). */
export function nowMs() {
  return clk.at(perf()) + ((ly.data && ly.data.offset) || 0);
}

const mine = (id, data) => {
  if (ly.id !== id || !data) return false;
  run(async () => null); // a slower lyrics_get for this song must not overwrite the choice
  ly.data = data;
  confirmed = data.offset || 0; confirmedSeq = offSeq;
  ly.loading = false;
  retried = true; // a manual choice replaces the automatic retry
  clearTimeout(retryTimer);
  requested = id;
  emit();
  return true;
};

/**
 * Shifts the offset by `deltaMs` (positive: lyrics earlier). The local value updates at once, so rapid
 * clicks accumulate; absolute values are sent and the newest response wins. On failure the last
 * server-confirmed value is restored (no rejection reaches the caller).
 */
export async function setOffset(deltaMs) {
  const id = ly.id, d = ly.data;
  if (!id || !d) return;
  d.offset = Math.max(-MAX_OFFSET, Math.min(MAX_OFFSET, (d.offset || 0) + deltaMs));
  const seq = ++offSeq;
  emit();
  try {
    const ms = await be.invoke('lyrics_set_offset', { id, ms: d.offset });
    if (ly.data !== d) return;
    if (seq >= confirmedSeq) { confirmed = ms; confirmedSeq = seq; }
    if (seq === offSeq) { d.offset = ms; emit(); }
  } catch (e) {
    console.error('lyrics_set_offset', e);
    if (ly.data === d && seq === offSeq) { d.offset = confirmed; emit(); }
  }
}

/** Uses a search result; rejects with the backend's i18n key on failure. */
export async function choose(source, extId) {
  const id = ly.id;
  if (!id) return;
  mine(id, await be.invoke('lyrics_choose', { id, source, extId }));
}

/** Uses a local lyrics file; rejects with an i18n key ("lyrics.badFile"). */
export async function useFile(path) {
  const id = ly.id;
  if (!id) return;
  mine(id, await be.invoke('lyrics_use_file', { id, path }));
}

const saveSettings = () => be.invoke('lyrics_set_settings', { netease: ly.netease, trans: ly.trans });

export async function setTrans(on) {
  ly.trans = !!on;
  emit();
  await saveSettings();
}

/** Switching the online source on/off refetches the current song when a view is open. */
export async function setNetease(on) {
  ly.netease = !!on;
  emit();
  await saveSettings();
  requested = null;
  if (active && ly.id) load(ly.id);
}
