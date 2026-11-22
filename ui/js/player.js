// Player bar. The queue and playback live in the backend; this module only mirrors
// `player://state` / `player://pos` and sends commands.
import { t } from './i18n.js';
import { api } from './api.js';
import { esc, mmss, trackSlider } from './util.js';
import { art, badges, artistName } from './views/songs.js';
import { pl, isSaved, cachedItems, loadItems, FAV } from './playlists.js';
import { saveSongs } from './savepicker.js';
import { toggleQueue } from './queue.js';
import { onSong, onPos, seekTo } from './lyrics.js';

const $ = (id) => document.getElementById(id);
const REPEAT_NEXT = { all: 'one', one: 'off', off: 'all' };
const REPEAT_KEY = { all: 'player.repeatAll', one: 'player.repeatOne', off: 'player.repeatOff' };

let ctx = null;
/** Last state from the backend. */
let ps = { id: null, playing: false, pos: 0, dur: 0, shuffle: false, repeat: 'all', volume: 100 };
/**
 * The page and filters the current queue was started from (for revealNow(true)), with `label`:
 * the page title then (the queue panel's "from <label>").
 */
let source = null;
let seekDrag = { dragging: () => false };
/** Ignore `player://pos` briefly after a seek: events already in flight carry the old spot. */
let holdPosUntil = 0;
let volDrag = { dragging: () => false };
let errId = null, errTimer = 0;

export const playerState = () => ps;

/** Name of the page the queue was started from, or '' when unknown. */
export function sourceLabel() {
  if (!source) return '';
  if (source.tab === 'fav') return t('fav.title');
  if (source.tab === 'pl') { const p = pl.list.find((x) => x.id === source.pl); return p ? p.name : source.label || ''; }
  return source.label || '';
}

/** Forget the source (playback started some other way, e.g. adding to an empty queue). */
export function clearSource() { source = null; }

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
  showSaved();
}

/** Heart next to the title: filled when the playing song is in favorites or a playlist. */
export function showSaved() {
  if (!ctx) return;
  const on = !!ps.id && isSaved(ps.id);
  $('plsaveicon').setAttribute('href', on ? '#i-heart-f' : '#i-heart');
  $('plsave').setAttribute('aria-pressed', String(on));
  $('plsave').classList.toggle('on', on);
  $('plsave').hidden = !songById(ps.id);
}

/** Highlights the playing row among the rendered rows. */
export function markRows() {
  document.querySelectorAll('#view .song').forEach((r) => r.classList.toggle('now', !!ps.id && r.dataset.id === ps.id));
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
  if (!volDrag.dragging()) showVolume(ps.volume);
  $('tdur').textContent = mmss(ps.dur);
  if (!seekDrag.dragging()) showPos(ps.pos);
  if (ps.id !== prevId) onSong(ps.id);
  onPos(ps.pos, playing);
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
  const h1 = document.querySelector('#view .hero h1');
  source = { tab: st.tab, pl: st.pl, label: h1 ? h1.textContent.trim() : '', plSort: st.plSort && { ...st.plSort }, artist: st.artist, album: st.album, sort: { ...st.sort }, q: st.q, quality: st.quality, root: st.root, pageQ: st.pageQ };
  hideError();
  call('player_play_list', { ids: list.map((s) => s.id), start: Math.max(0, Math.min(list.length - 1, start)), shuffle: !!shuffle });
}

/** Jumps to `secs` (slider, lyric line): the UI and the lyric clock follow at once. */
export function seekPlayer(secs) {
  if (ps.dur) secs = Math.min(secs, ps.dur);
  holdPosUntil = performance.now() + 600;
  showPos(secs);
  seekTo(secs, ps.playing);
  if (ps.dur) call('player_seek', { secs });
}

export const toggle = () => { if (ps.id) call('player_toggle'); };

/**
 * Brings the playing song's row into view. Without `force` it scrolls only when the row is
 * outside the visible area (used on song change). With `force` (clicking the player bar) it
 * always centres the row, first going back to the page the queue was started from (songs,
 * playlist or favorites) if the song is not in the current list, and flashes the row. Without a
 * known source it only looks in the current list.
 */
export function revealNow(force) {
  const id = ps.id;
  if (!id) return;
  const find = () => (ctx.view && ctx.view.vlist ? ctx.list.findIndex((s) => s.id === id) : -1);
  let i = find();
  if (i < 0 && force && source) {
    ctx.showSource(source);
    i = find();
    const pid = source.tab === 'fav' ? FAV : source.tab === 'pl' ? source.pl : null;
    if (i < 0 && pid != null && !cachedItems(pid)) {
      // the page shows "loading" and renders once its items arrive (that render runs first)
      const tab = source.tab;
      loadItems(pid).then(() => {
        if (ps.id === id && ctx.state.tab === tab && (tab === 'fav' || ctx.state.pl === pid)) scrollToRow(find(), id, true);
      }, () => {});
      return;
    }
  }
  scrollToRow(i, id, force);
}

function scrollToRow(i, id, force) {
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
  $('plsave').onclick = () => { if (songById(ps.id)) saveSongs([ps.id], $('plsave'), ctx); };
  $('queuebtn').onclick = () => toggleQueue();
  $('next').onclick = () => call('player_next');
  $('prev').onclick = () => call('player_prev');
  $('shuffle').onclick = () => call('player_set_shuffle', { on: !ps.shuffle });
  $('repeat').onclick = () => call('player_set_repeat', { mode: REPEAT_NEXT[ps.repeat] || 'all' });

  // slider position of the playing spot (what showPos would set)
  const seekValue = () => (ps.dur ? Math.round(Math.min(1000, Math.max(0, (ps.pos / ps.dur) * 1000))) : 0);
  const seek = $('seek');
  seekDrag = trackSlider(seek, {
    current: seekValue,
    preview(v) {
      seek.style.setProperty('--p', v / 10 + '%');
      $('tcur').textContent = mmss((v / 1000) * (ps.dur || 0));
    },
    end(v) {
      if (v == null) { showPos(ps.pos); return; } // dragged back to where it started
      seekPlayer((v / 1000) * (ps.dur || 0));
    },
  });

  let volTimer = 0;
  /** last volume sent during this drag (its player://state may not be back yet) */
  let volSent = null;
  const vol = $('vol');
  volDrag = trackSlider(vol, {
    current: () => (volSent ?? ps.volume),
    preview(v) {
      showVolume(v);
      clearTimeout(volTimer);
      volTimer = setTimeout(() => { volSent = v; call('player_set_volume', { volume: v }); }, 80);
    },
    end(v) {
      clearTimeout(volTimer);
      volSent = null;
      if (v == null) showVolume(ps.volume);
      else call('player_set_volume', { volume: v });
    },
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
    if (seekDrag.dragging() || performance.now() < holdPosUntil) return;
    ps.pos = pos;
    onPos(pos, ps.playing);
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
