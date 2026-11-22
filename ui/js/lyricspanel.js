// Lyrics drawer in the main window. The line view, frame loop and tools live in lyricsview.js.
import { ly } from './lyrics.js';
import { createLyricsView } from './lyricsview.js';
import { closeQueue } from './queue.js';
import { menuOpen } from './menu.js';
import { pickerOpen } from './savepicker.js';
import { initLyricSearch } from './lyricsearch.js';
import { nowPlayingOpen, closeNowPlaying } from './nowplaying.js';

const $ = (id) => document.getElementById(id);
let ctx = null;
let isOpen = false;
let lyv = null;

function showSong() {
  const s = ly.id && ctx.songsById.get(ly.id);
  $('lysong').textContent = s ? s.t : '';
}

export function openLyrics() {
  if (isOpen) return;
  closeQueue();
  isOpen = true;
  $('lyrics').hidden = false;
  $('lyricsbtn').setAttribute('aria-pressed', 'true');
  lyv.show();
  $('lyricsclose').focus({ preventScroll: true });
}

export function closeLyrics() {
  if (!isOpen) return;
  isOpen = false;
  const had = $('lyrics').contains(document.activeElement);
  lyv.hide();
  $('lyrics').hidden = true;
  $('lyricsbtn').setAttribute('aria-pressed', 'false');
  if (had) $('lyricsbtn').focus({ preventScroll: true });
}

/** On the full-screen page the lyrics button and L mean "back": that page already shows the lyrics. */
export function toggleLyrics() {
  if (nowPlayingOpen()) closeNowPlaying();
  else if (isOpen) closeLyrics();
  else openLyrics();
}
export const lyricsOpen = () => isOpen;

export function initLyricsPanel(c) {
  ctx = c;
  initLyricSearch(c);
  lyv = createLyricsView('ly', { onRender: showSong });
  $('lyricsbtn').onclick = toggleLyrics;
  $('lyricsclose').onclick = closeLyrics;

  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const typing = /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName) || document.activeElement.isContentEditable;
    const busy = typing || !!document.querySelector('dialog[open]') || menuOpen() || pickerOpen();
    if ((e.key === 'l' || e.key === 'L') && !busy && !e.repeat) { e.preventDefault(); toggleLyrics(); }
    else if (e.key === 'Escape' && isOpen && !busy) closeLyrics();
  });
}
