// Player bar. The queue and playback live in the backend; this module only mirrors
// `player://state` / `player://pos` and sends commands.
import { t } from './i18n.js';
import { api } from './api.js';
import { esc, mmss } from './util.js';
import { art, badges, artistName } from './views/songs.js';

const $ = (id) => document.getElementById(id);
const REPEAT_NEXT = { all: 'one', one: 'off', off: 'all' };
const REPEAT_KEY = { all: 'player.repeatAll', one: 'player.repeatOne', off: 'player.repeatOff' };

let ctx = null;
/** Last state from the backend. */
let ps = { id: null, playing: false, pos: 0, dur: 0, shuffle: false, repeat: 'all', volume: 100 };
/** The songs-page filters the current queue was started from (for revealNow(true)). */
let source = null;
let seeking = false;
/** Ignore `player://pos` briefly after a seek: events already in flight carry the old spot. */
let holdPosUntil = 0;
let volDragging = false;
let errId = null, errTimer = 0;

export const playerState = () => ps;

const songById = (id) => (id && ctx.lib.songs.find((s) => s.id === id)) || null;

function setPlayerHeight() {
  const p = $('player');
  document.documentElement.style.setProperty('--player-h', p.hidden ? '0px' : p.offsetHeight + 'px');
}

function showPos(pos) {
  const d = ps.dur || 0;
  const v = d ? Math.min(1000, Math.max(0, (pos / d) * 1000)) : 0;
  $('seek').value = v;
  $('seek').style.setProperty('--p', v / 10 + '%');
  $('tcur').textContent = mmss(pos);
}

function showVolume(v) {
  $('vol').value = v;
  $('vol').style.setProperty('--p', v + '%');
}

function renderNow() {
  const s = songById(ps.id);
  $('plcv').innerHTML = s ? art(s, 'note', 'sm') : '';
  $('nptitle').innerHTML = s ? `${esc(s.t)} ${badges(s)}` : '&mdash;';
  $('npartist').textContent = s ? [artistName(s.a), s.b].filter(Boolean).join(' · ') : '';
}

/** Highlights the playing row among the rendered rows. */
export function markRows() {
  document.querySelectorAll('.song').forEach((r) => r.classList.toggle('now', !!ps.id && r.dataset.id === ps.id));
}

function applyState(st) {
  const prevId = ps.id;
  ps = { ...ps, ...st };
  ctx.nowId = ps.id;
  const bar = $('player');
  const wasHidden = bar.hidden;
  bar.hidden = !ps.id;
  const playing = !!ps.playing;
  $('ppicon').setAttribute('href', playing ? '#i-pause' : '#i-play');
  $('playpause').setAttribute('aria-label', t(playing ? 'player.pause' : 'player.play'));
  document.body.classList.toggle('playing', playing);
  $('shuffle').setAttribute('aria-pressed', String(!!ps.shuffle));
  const rep = REPEAT_KEY[ps.repeat] ? ps.repeat : 'all';
  const rb = $('repeat');
  rb.setAttribute('aria-pressed', String(rep !== 'off'));
  rb.dataset.mode = rep;
  rb.title = t(REPEAT_KEY[rep]);
  rb.setAttribute('aria-label', t(REPEAT_KEY[rep]));
  $('repicon').setAttribute('href', rep === 'one' ? '#i-repeat-one' : '#i-repeat');
  if (!volDragging) showVolume(ps.volume);
  $('tdur').textContent = mmss(ps.dur);
  if (!seeking) showPos(ps.pos);
  if (ps.id !== prevId) {
    renderNow();
    markRows();
    if (ps.id) revealNow(false);
  }
  if (wasHidden !== bar.hidden) setPlayerHeight();
}

function showError(id, reason) {
  const s = songById(id);
  const head = t('player.error', { title: s ? s.t : '' });
  $('perr').textContent = reason ? t('player.errorReason', { error: head, reason: t(reason) }) : head;
  $('perr').hidden = false;
  errId = id;
  clearTimeout(errTimer);
  errTimer = setTimeout(hideError, 10000);
  setPlayerHeight();
}

function hideError() {
  if ($('perr').hidden) return;
  $('perr').hidden = true;
  errId = null;
  clearTimeout(errTimer);
  setPlayerHeight();
}

const call = (cmd, args) => api.invoke(cmd, args).catch((e) => console.error(cmd, e));

/** Starts `list` (already filtered to playable songs) at `start`, remembering where it came from. */
export function playList(list, start, shuffle) {
  if (!list.length) return;
  const st = ctx.state;
  source = { artist: st.artist, album: st.album, sort: { ...st.sort }, q: st.q, quality: st.quality, root: st.root };
  hideError();
  call('player_play_list', { ids: list.map((s) => s.id), start: Math.max(0, Math.min(list.length - 1, start)), shuffle: !!shuffle });
}

export const toggle = () => { if (ps.id) call('player_toggle'); };

/**
 * Brings the playing song's row into view. Without `force` it scrolls only when the row is
 * outside the visible area (used on song change). With `force` (clicking the player bar) it
 * always centres the row, first going back to the page the queue was started from if the song
 * is not in the current list, and flashes the row.
 */
export function revealNow(force) {
  const id = ps.id;
  if (!id) return;
  let i = ctx.view && ctx.view.vlist ? ctx.list.findIndex((s) => s.id === id) : -1;
  if (i < 0 && force && source) {
    ctx.showSource(source);
    i = ctx.view && ctx.view.vlist ? ctx.list.findIndex((s) => s.id === id) : -1;
  }
  if (i < 0) return;
  const vl = ctx.view.vlist;
  vl.scrollToIndex(i, force ? 'center' : 'reveal', 'smooth');
  if (!force) return;
  // the row may only be rendered once the smooth scroll gets there
  const t0 = performance.now();
  const flash = () => {
    const el = ctx.view && ctx.view.vlist === vl ? vl.node(i) : null;
    const row = el && (el.classList.contains('song') ? el : el.querySelector('.song'));
    if (row && row.dataset.id === id) {
      row.classList.remove('flash');
      void row.offsetWidth;
      row.classList.add('flash');
    } else if (performance.now() - t0 < 1500) requestAnimationFrame(flash);
  };
  requestAnimationFrame(flash);
}

export function initPlayer(c) {
  ctx = c;
  $('plnow').onclick = () => revealNow(true);
  $('plnow').onkeydown = (e) => { if (e.key === 'Enter') { e.preventDefault(); revealNow(true); } };
  $('playpause').onclick = toggle;
  $('next').onclick = () => call('player_next');
  $('prev').onclick = () => call('player_prev');
  $('shuffle').onclick = () => call('player_set_shuffle', { on: !ps.shuffle });
  $('repeat').onclick = () => call('player_set_repeat', { mode: REPEAT_NEXT[ps.repeat] || 'all' });

  const seek = $('seek');
  seek.addEventListener('input', () => {
    seeking = true;
    seek.style.setProperty('--p', seek.value / 10 + '%');
    $('tcur').textContent = mmss((seek.value / 1000) * (ps.dur || 0));
  });
  seek.addEventListener('change', () => {
    const secs = (seek.value / 1000) * (ps.dur || 0);
    seeking = false;
    holdPosUntil = performance.now() + 600;
    showPos(secs);
    if (ps.dur) call('player_seek', { secs });
  });

  let volTimer = 0;
  const vol = $('vol');
  vol.addEventListener('input', () => {
    volDragging = true;
    showVolume(+vol.value);
    clearTimeout(volTimer);
    volTimer = setTimeout(() => call('player_set_volume', { volume: +vol.value }), 80);
  });
  vol.addEventListener('change', () => {
    clearTimeout(volTimer);
    volDragging = false;
    call('player_set_volume', { volume: +vol.value });
  });

  document.addEventListener('keydown', (e) => {
    if (e.key !== ' ' || e.ctrlKey || e.altKey || e.metaKey) return;
    const typing = /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName) || $('settings').open;
    if (typing || !ps.id || (e.target instanceof Element && e.target.closest('button'))) return;
    e.preventDefault();
    toggle();
  });
  window.addEventListener('resize', () => { if (!$('player').hidden) setPlayerHeight(); });

  api.listen('player://state', applyState);
  api.listen('player://pos', ({ pos }) => {
    if (seeking || performance.now() < holdPosUntil) return;
    ps.pos = pos;
    showPos(pos);
    if (errId && errId !== ps.id) hideError();
  });
  api.listen('player://error', ({ id, reason }) => showError(id, reason));
  return api.invoke('player_state').then(applyState).catch((e) => console.error('player_state', e));
}

/** Library reloaded: refresh the now-playing labels (titles, covers may have changed). */
export function libraryChanged() {
  if (ps.id) renderNow();
}
