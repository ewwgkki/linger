// Full-screen play page: spinning record on the left, lyrics on the right, backdrop from the cover
// colour. It sits under the player bar, so the transport stays usable. Opening it hides the lyrics or
// queue drawer and closing it brings that drawer back, so only one lyrics view ever drives frames.
import { t } from './i18n.js';
import { api } from './api.js';
import { ly, subscribe, isPlaying, onClock } from './lyrics.js';
import { backdropColors } from './lyricsmodel.js';
import { createLyricsView } from './lyricsview.js';
import { art, artistName } from './views/songs.js';
import { lyricsOpen, openLyrics, closeLyrics } from './lyricspanel.js';
import { queueOpen, openQueue, closeQueue } from './queue.js';
import { menuOpen } from './menu.js';
import { pickerOpen } from './savepicker.js';

const $ = (id) => document.getElementById(id);
let ctx = null;
let isOpen = false;
let lyv = null;
/** drawer to bring back on close: 'lyrics' | 'queue' | null */
let prevDrawer = null;
/** song the title, cover and colour were last shown for */
let shownId = null;
let shownCover = null;
let colorSeq = 0;

function applyColors(rgb) {
  const c = backdropColors(rgb);
  const st = document.body.style;
  st.setProperty('--np-top', c.top);
  st.setProperty('--np-bottom', c.bottom);
  st.setProperty('--np-ink', c.ink);
  st.setProperty('--np-mute', c.mute);
  st.setProperty('--np-off', c.off);
}

function clearColors() {
  for (const k of ['--np-top', '--np-bottom', '--np-ink', '--np-mute', '--np-off']) document.body.style.removeProperty(k);
}

/** A slow answer for an earlier song (or after closing) is dropped by the sequence number. */
async function loadColor(id) {
  const seq = ++colorSeq;
  let rgb = null;
  try {
    rgb = await api.invoke('cover_color', { id });
  } catch (e) {
    console.error('cover_color', e);
  }
  if (isOpen && seq === colorSeq && ly.id === id) applyColors(rgb);
}

function paintSong() {
  const s = ly.id && ctx.songsById.get(ly.id);
  $('npltitle').textContent = s ? s.t : '';
  $('nplartist').textContent = s ? [artistName(s.a), s.b].filter(Boolean).join(' · ') : '';
  const cover = s ? art(s, 'note', 'round') : '';
  if (cover !== shownCover) { shownCover = cover; $('nplcv').innerHTML = cover; } // no flicker on a rescan
}

function showSong() {
  if (ly.id === shownId) return;
  shownId = ly.id;
  paintSong();
  if (ly.id) loadColor(ly.id);
}

/** The library was reloaded: titles and covers may have changed. */
export function refreshNowPlaying() {
  if (!isOpen) return;
  paintSong();
  if (ly.id) loadColor(ly.id);
}

/** The user opened or closed a drawer on the page: closing the page must not undo that. */
export function forgetDrawer() {
  prevDrawer = null;
}

/** The record turns only while playing and visible (reduced motion is handled in the stylesheet). */
function syncDisc() {
  $('npldisc').classList.toggle('spin', isOpen && isPlaying() && !document.hidden);
}

export const nowPlayingOpen = () => isOpen;

export function openNowPlaying() {
  if (isOpen || !ly.id) return;
  const drawer = lyricsOpen() ? 'lyrics' : queueOpen() ? 'queue' : null;
  isOpen = true;
  applyColors(null); // default palette until the cover colour arrives
  // hold the new view before the drawer lets go, so the shared count never reaches zero
  $('nowplaying').hidden = false;
  document.body.classList.add('np-open');
  $('app').inert = true;
  lyv.show();
  closeLyrics();
  closeQueue();
  prevDrawer = drawer; // after the closes above, which count as drawer changes
  $('lyricsbtn').setAttribute('aria-pressed', 'true');
  $('plcover').setAttribute('aria-expanded', 'true');
  $('plcover').setAttribute('aria-label', t('np.close'));
  $('plcover').title = t('np.close');
  syncDisc();
  $('nplclose').focus({ preventScroll: true });
}

export function closeNowPlaying() {
  if (!isOpen) return;
  isOpen = false;
  colorSeq++;
  shownId = null;
  const had = $('nowplaying').contains(document.activeElement);
  const back = prevDrawer;
  prevDrawer = null;
  // the previous drawer returns (before the view lets go), unless the user changed drawers on the page
  if (back === 'lyrics') openLyrics();
  else if (back === 'queue') openQueue();
  lyv.hide();
  $('nowplaying').hidden = true;
  document.body.classList.remove('np-open');
  $('app').inert = false;
  clearColors();
  syncDisc();
  $('lyricsbtn').setAttribute('aria-pressed', String(lyricsOpen()));
  $('plcover').setAttribute('aria-expanded', 'false');
  $('plcover').setAttribute('aria-label', t('np.open'));
  $('plcover').title = t('np.open');
  if (had && !back && !$('player').hidden) $('plcover').focus({ preventScroll: true });
}

const toggleNowPlaying = () => (isOpen ? closeNowPlaying() : openNowPlaying());

export function initNowPlaying(c) {
  ctx = c;
  lyv = createLyricsView('npl', { onRender: showSong });
  $('plcover').onclick = toggleNowPlaying;
  $('nplclose').onclick = closeNowPlaying;

  // the page is not a scroll container: a wheel outside the lyrics must not scroll the list behind it
  $('nowplaying').addEventListener('wheel', (e) => { if (!e.target.closest('.ly-scroll')) e.preventDefault(); }, { passive: false });

  subscribe(() => { if (isOpen && !ly.id) closeNowPlaying(); });
  onClock(syncDisc);
  document.addEventListener('visibilitychange', syncDisc);

  // capture: runs before the drawers' and the page's own Esc handlers
  document.addEventListener('keydown', (e) => {
    if (!isOpen || e.key !== 'Escape' || e.ctrlKey || e.metaKey || e.altKey) return;
    const a = document.activeElement;
    const typing = (/INPUT|SELECT|TEXTAREA/.test(a.tagName) && a.type !== 'range') || a.isContentEditable;
    if (typing || document.querySelector('dialog[open]') || menuOpen() || pickerOpen()) return;
    if (queueOpen()) return; // the queue drawer on top closes first
    e.preventDefault();
    e.stopImmediatePropagation();
    closeNowPlaying();
  }, true);
}
