// Lyrics drawer in the main window: karaoke highlight, smooth scrolling, offset and source controls.
// `renderLines` is the shared line view (the full-screen page uses it too). The frame loop runs only
// while the drawer is open, the window is visible and the song is playing; each frame touches only
// the current line's CSS variables, so long lyrics cost no layout.
import { t } from './i18n.js';
import { esc, icon } from './util.js';
import { ly, subscribe, holdView, reload, nowMs, isPlaying, onClock, setOffset, setTrans } from './lyrics.js';
import { lineIndex, wordFill, lineFill, seekTarget, scrollGate } from './lyricsmodel.js';
import { seekPlayer } from './player.js';
import { closeQueue } from './queue.js';
import { menuOpen } from './menu.js';
import { pickerOpen } from './savepicker.js';
import { toast } from './settings.js';
import { initLyricSearch, openLyricSearch } from './lyricsearch.js';

const $ = (id) => document.getElementById(id);
const MANUAL_SCROLL_MS = 4000;
const OFFSET_STEP = 500;
const calm = () => matchMedia('(prefers-reduced-motion:reduce)').matches;

function lineHtml(line, i, showTrans) {
  const timed = line.start != null;
  const blank = !line.text.trim();
  const words = line.words || [];
  const text = blank ? '♪'
    : words.length ? words.map((w) => `<span class="ly-w">${esc(w.text)}</span>`).join('')
    : esc(line.text);
  const tr = showTrans && line.trans ? `<span class="ly-tr">${esc(line.trans)}</span>` : '';
  const cls = `ly-line${words.length ? ' has-w' : ''}${blank ? ' blank' : ''}`;
  const inner = `<span class="ly-text">${text}</span>${tr}`;
  return timed
    ? `<button type="button" class="${cls}" data-i="${i}" tabindex="-1">${inner}</button>`
    : `<p class="${cls} plain">${inner}</p>`;
}

/**
 * Renders `data.lines` into the scroll container `el`. opts: `trans` (show translations),
 * `onSeek(line, i)` (a timed line was activated). Returns the view:
 * `tick(ms)` paints the highlight and returns true when the current line changed;
 * `center(smooth)` scrolls the current line to the middle.
 */
export function renderLines(el, data, opts = {}) {
  const lines = data.lines || [];
  const timed = data.kind !== 'plain' && lines.length > 0 && lines[0].start != null;
  el.classList.add('ly-scroll');
  el.innerHTML = `<div class="ly-list${timed ? '' : ' is-plain'}">${lines.map((l, i) => lineHtml(l, i, !!opts.trans)).join('')}</div>`;
  const nodes = [...el.querySelectorAll('.ly-line')];
  let cur = -1, wordEls = [], textEl = null, last = [];

  const setFill = (node, k, v) => {
    const f = Math.round(v * 1000) / 1000;
    if (last[k] === f) return;
    last[k] = f;
    node.style.setProperty('--fill', f);
  };
  const setCurrent = (i) => {
    const old = nodes[cur];
    if (old) { old.classList.remove('cur'); old.tabIndex = -1; old.removeAttribute('aria-current'); }
    cur = i;
    last = [];
    const node = nodes[i];
    wordEls = node ? [...node.querySelectorAll('.ly-w')] : [];
    textEl = node ? node.querySelector('.ly-text') : null;
    if (node) { node.classList.add('cur'); node.tabIndex = 0; node.setAttribute('aria-current', 'true'); }
    else if (nodes[0] && timed) nodes[0].tabIndex = 0; // keeps the list reachable by keyboard before the first line
  };

  el.onclick = (e) => {
    const b = timed && e.target.closest('.ly-line[data-i]');
    if (b && opts.onSeek) opts.onSeek(lines[+b.dataset.i], +b.dataset.i);
  };
  el.onkeydown = (e) => {
    if (!timed || (e.key !== 'ArrowDown' && e.key !== 'ArrowUp')) return;
    const b = e.target.closest('.ly-line[data-i]');
    const next = b && nodes[+b.dataset.i + (e.key === 'ArrowDown' ? 1 : -1)];
    if (next) { e.preventDefault(); next.focus(); }
  };
  if (timed) setCurrent(-1);

  return {
    timed,
    tick(ms) {
      if (!timed) return false;
      const i = lineIndex(lines, ms);
      const changed = i !== cur;
      if (changed) setCurrent(i);
      if (cur < 0) return changed;
      const line = lines[cur];
      if (wordEls.length) line.words.forEach((w, k) => setFill(wordEls[k], k, wordFill(w, ms)));
      else if (textEl) setFill(textEl, 0, lineFill(lines, cur, ms));
      return changed;
    },
    center(smooth) {
      const node = nodes[cur] || (timed ? nodes[0] : null);
      if (!node) return;
      const top = node.offsetTop + node.offsetHeight / 2 - el.clientHeight / 2;
      el.scrollTo({ top: Math.max(0, top), behavior: smooth && !calm() ? 'smooth' : 'auto' });
    },
  };
}

// ---------- drawer ----------
let ctx = null;
let isOpen = false;
let release = null;
let view = null;
/** what the current view was built from */
let shownData = null, shownTrans = null;
let raf = 0, gateTimer = 0;
/** the view should return to the current line as soon as the gate allows */
let recenter = false;
let pointerHeld = false;
const gate = scrollGate(MANUAL_SCROLL_MS);

function stopFrames() {
  if (raf) cancelAnimationFrame(raf);
  raf = 0;
}

const running = () => isOpen && !document.hidden && !!view && view.timed && isPlaying();

function frame() {
  raf = 0;
  if (!isOpen || !view) return;
  if (view.tick(nowMs())) recenter = true;
  if (recenter && gate.allowed(performance.now())) {
    recenter = false;
    view.center(true);
  }
  if (running()) raf = requestAnimationFrame(frame);
}

/** Schedules a frame; the loop keeps going by itself only while playing. */
function kick() {
  if (isOpen && !document.hidden && view && view.timed && !raf) raf = requestAnimationFrame(frame);
}

function manualScroll() {
  gate.touch(performance.now());
  recenter = true;
  clearTimeout(gateTimer);
  gateTimer = setTimeout(kick, MANUAL_SCROLL_MS + 20); // also returns while paused
}

function seekLine(line) {
  const target = seekTarget(line, (ly.data && ly.data.offset) || 0);
  if (target == null) return;
  // half a millisecond past the stamp: the conversion to seconds must not land just before the line
  seekPlayer((target + 0.5) / 1000);
  gate.clear();
  recenter = true;
  kick();
}

const offsetText = (ms) => t('lyrics.offset', { v: `${ms > 0 ? '+' : ms < 0 ? '-' : ''}${(Math.abs(ms) / 1000).toFixed(1)}` });
const sourceName = (s) => (['netease', 'lrclib', 'embedded', 'file', 'manual'].includes(s) ? t(`lyrics.src.${s}`) : '');

function showState(kind) {
  const box = $('lystate');
  box.hidden = !kind;
  if (!kind) return;
  const action = kind === 'none' ? 'lyrics.search' : kind === 'retry' ? 'lyrics.retryBtn' : '';
  box.dataset.kind = kind;
  $('lystatemsg').textContent = t({ idle: 'lyrics.noSong', loading: 'lyrics.loading', none: 'lyrics.none', retry: 'lyrics.retry' }[kind]);
  $('lystatebtn').hidden = !action;
  if (action) $('lystatebtn').textContent = t(action);
}

function clearLines() {
  stopFrames();
  view = null;
  shownData = null;
  $('lylines').hidden = true;
  $('lylines').textContent = '';
}

function showSong() {
  const s = ly.id && ctx.songsById.get(ly.id);
  $('lysong').textContent = s ? s.t : '';
}

function render() {
  if (!isOpen) return;
  showSong();
  const d = ly.data;
  const hasLines = !!d && d.status === 'ok' && d.lines.length > 0;
  if (!hasLines) {
    clearLines();
    $('lytools').hidden = true;
    showState(!ly.id ? 'idle' : ly.loading ? 'loading' : !d || d.status === 'retry' ? 'retry' : 'none');
    return;
  }
  showState(null);
  const hasTrans = d.lines.some((l) => l.trans);
  const trans = hasTrans && ly.trans;
  if (d !== shownData || trans !== shownTrans) {
    stopFrames();
    shownData = d;
    shownTrans = trans;
    $('lylines').hidden = false;
    view = renderLines($('lylines'), d, { trans, onSeek: seekLine });
    view.tick(nowMs());
    view.center(false);
    recenter = false;
  }
  $('lytools').hidden = false;
  $('lyoffsetbox').hidden = !view.timed;
  $('lyoffset').textContent = offsetText(d.offset || 0);
  $('lytrans').hidden = !hasTrans;
  $('lytrans').setAttribute('aria-pressed', String(!!ly.trans));
  const src = sourceName(d.source);
  $('lysrc').textContent = src ? t('lyrics.sourceLabel', { name: src }) : '';
  kick();
}

export function openLyrics() {
  if (isOpen) return;
  closeQueue();
  isOpen = true;
  $('lyrics').hidden = false;
  $('lyricsbtn').setAttribute('aria-pressed', 'true');
  release = holdView();
  render();
  if (view) view.center(false); // the drawer has its real size only now
  $('lyricsclose').focus({ preventScroll: true });
}

export function closeLyrics() {
  if (!isOpen) return;
  isOpen = false;
  stopFrames();
  clearTimeout(gateTimer);
  gate.clear();
  recenter = false;
  if (release) release();
  release = null;
  const had = $('lyrics').contains(document.activeElement);
  $('lyrics').hidden = true;
  $('lyricsbtn').setAttribute('aria-pressed', 'false');
  clearLines(); // rebuilt on open, so a closed drawer holds no nodes for a long song
  if (had) $('lyricsbtn').focus({ preventScroll: true });
}

export const toggleLyrics = () => (isOpen ? closeLyrics() : openLyrics());
export const lyricsOpen = () => isOpen;

export function initLyricsPanel(c) {
  ctx = c;
  initLyricSearch(c);
  $('lyricsbtn').onclick = toggleLyrics;
  $('lyricsclose').onclick = closeLyrics;
  $('lystatebtn').onclick = () => {
    if ($('lystate').dataset.kind === 'retry') reload();
    else openLyricSearch();
  };
  $('lyearlier').onclick = () => setOffset(OFFSET_STEP);
  $('lylater').onclick = () => setOffset(-OFFSET_STEP);
  $('lytrans').onclick = () => setTrans(!ly.trans).catch((e) => {
    console.error('lyrics_set_settings', e);
    toast(t('lyrics.saveError'), { icon: 'x', autoHide: 6000 });
  });
  $('lysearch').onclick = openLyricSearch;
  $('lystateicon').innerHTML = icon('lyrics');

  const box = $('lylines');
  box.addEventListener('wheel', manualScroll, { passive: true });
  box.addEventListener('touchmove', manualScroll, { passive: true });
  box.addEventListener('keydown', (e) => { if (['PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown'].includes(e.key)) manualScroll(); });
  // dragging the scrollbar: the press lands on the container itself
  box.addEventListener('pointerdown', (e) => { pointerHeld = e.target === box; });
  window.addEventListener('pointerup', () => { pointerHeld = false; });
  window.addEventListener('pointercancel', () => { pointerHeld = false; });
  box.addEventListener('scroll', () => { if (pointerHeld) manualScroll(); }, { passive: true });
  new ResizeObserver(() => { if (isOpen && view && gate.allowed(performance.now())) view.center(false); }).observe(box);

  subscribe(render);
  onClock(kick);
  document.addEventListener('visibilitychange', () => { if (document.hidden) stopFrames(); else kick(); });

  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const typing = /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName) || document.activeElement.isContentEditable;
    const busy = typing || !!document.querySelector('dialog[open]') || menuOpen() || pickerOpen();
    if ((e.key === 'l' || e.key === 'L') && !busy && !e.repeat) { e.preventDefault(); toggleLyrics(); }
    else if (e.key === 'Escape' && isOpen && !busy) closeLyrics();
  });
}
