// Lyrics store: data for the playing song plus the playback clock. No DOM and no
// requestAnimationFrame here; the lyrics views (panel, full screen) run their own frame loop only
// while open and read `nowMs()` from it. Wired from player.js (onSong / onPos) and app.js (initLyrics).
import { api } from './api.js';
import { clock, latest, STALE } from './lyricsmodel.js';

/** id: playing song; data: last `lyrics_get` result (null until loaded); loading: request in flight. */
export const ly = { id: null, data: null, loading: false, trans: true, netease: true };

const RETRY_MS = 30000;
const MAX_OFFSET = 30000;
const subs = new Set();
const run = latest();
const clk = clock();
const perf = () => (globalThis.performance ? performance.now() : Date.now());
let active = false;
let retryTimer = 0;
let retried = false;
/** song id whose data was requested (so setActive does not refetch) */
let requested = null;

const emit = () => { for (const fn of [...subs]) { try { fn(ly); } catch (e) { console.error('lyrics subscriber', e); } } };

/** Calls `fn(ly)` on every data change; returns an unsubscribe function. */
export function subscribe(fn) {
  subs.add(fn);
  return () => subs.delete(fn);
}

export function initLyrics() {
  return api.invoke('lyrics_settings').then((s) => {
    if (s) { ly.netease = !!s.netease; ly.trans = !!s.trans; emit(); }
  }).catch((e) => console.error('lyrics_settings', e));
}

async function load(id) {
  requested = id;
  ly.loading = true;
  emit();
  let data = null;
  try {
    data = await run(() => api.invoke('lyrics_get', { id }));
  } catch (e) {
    console.error('lyrics_get', e);
  }
  if (data === STALE || ly.id !== id) return; // a later request or song change owns the state now
  ly.loading = false;
  ly.data = data;
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

/** Playback position in ms including the song's lyric offset (positive offset = lyrics earlier). */
export function nowMs() {
  return clk.at(perf()) + ((ly.data && ly.data.offset) || 0);
}

const mine = (id, data) => {
  if (ly.id !== id || !data) return false;
  ly.data = data;
  ly.loading = false;
  retried = true; // a manual choice replaces the automatic retry
  clearTimeout(retryTimer);
  requested = id;
  emit();
  return true;
};

/** Shifts the offset by `deltaMs` (positive: lyrics earlier); stores the absolute value. */
export async function setOffset(deltaMs) {
  const id = ly.id;
  if (!id || !ly.data) return;
  const want = Math.max(-MAX_OFFSET, Math.min(MAX_OFFSET, (ly.data.offset || 0) + deltaMs));
  const ms = await api.invoke('lyrics_set_offset', { id, ms: want });
  if (ly.id === id && ly.data) { ly.data.offset = ms; emit(); }
}

/** Uses a search result; rejects with the backend's i18n key on failure. */
export async function choose(source, extId) {
  const id = ly.id;
  if (!id) return;
  mine(id, await api.invoke('lyrics_choose', { id, source, extId }));
}

/** Uses a local lyrics file; rejects with an i18n key ("lyrics.badFile"). */
export async function useFile(path) {
  const id = ly.id;
  if (!id) return;
  mine(id, await api.invoke('lyrics_use_file', { id, path }));
}

const saveSettings = () => api.invoke('lyrics_set_settings', { netease: ly.netease, trans: ly.trans });

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
