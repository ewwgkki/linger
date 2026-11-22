// Shared lyrics view: the line list (karaoke highlight, click to seek) plus the frame loop, auto-scroll
// and tools around it. The main-window drawer and the full-screen page each own one instance; an
// instance does work only between show() and hide(), so only the visible one drives frames. Each frame
// touches only the current line's CSS variables, so long lyrics cost no layout.
import { t } from './i18n.js';
import { esc, icon } from './util.js';
import { ly, subscribe, holdView, reload, nowMs, isPlaying, onClock, setOffset, setTrans } from './lyrics.js';
import { lineIndex, wordFill, lineFill, seekTarget, scrollGate } from './lyricsmodel.js';
import { seekPlayer } from './player.js';
import { toast } from './settings.js';
import { openLyricSearch } from './lyricsearch.js';

const MANUAL_SCROLL_MS = 4000;
const OFFSET_STEP = 500;
const calm = () => matchMedia('(prefers-reduced-motion:reduce)').matches;
const offsetText = (ms) => t('lyrics.offset', { v: `${ms > 0 ? '+' : ms < 0 ? '-' : ''}${(Math.abs(ms) / 1000).toFixed(1)}` });
const sourceName = (s) => (['netease', 'lrclib', 'embedded', 'file', 'manual'].includes(s) ? t(`lyrics.src.${s}`) : '');

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

/**
 * Binds a view to the elements `<prefix>state`, `statemsg`, `stateicon`, `statebtn`, `lines`, `tools`,
 * `offsetbox`, `offset`, `earlier`, `later`, `trans`, `src` and `search`. `onRender` runs on every
 * store change while shown. Returns `{ show, hide, shown }`.
 */
export function createLyricsView(prefix, { onRender } = {}) {
  const $ = (name) => document.getElementById(prefix + name);
  const box = $('lines');
  let on = false, release = null, view = null;
  /** what the current view was built from */
  let shownData = null, shownTrans = null;
  let raf = 0, gateTimer = 0;
  /** the view should return to the current line as soon as the gate allows */
  let recenter = false;
  let pointerHeld = false;
  const gate = scrollGate(MANUAL_SCROLL_MS);

  const stopFrames = () => {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  };
  const running = () => on && !document.hidden && !!view && view.timed && isPlaying();

  function frame() {
    raf = 0;
    if (!on || !view) return;
    if (view.tick(nowMs())) recenter = true;
    if (recenter && gate.allowed(performance.now())) {
      recenter = false;
      view.center(true);
    }
    if (running()) raf = requestAnimationFrame(frame);
  }

  /** Schedules a frame; the loop keeps going by itself only while playing. */
  function kick() {
    if (on && !document.hidden && view && view.timed && !raf) raf = requestAnimationFrame(frame);
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

  function showState(kind) {
    const el = $('state');
    el.hidden = !kind;
    if (!kind) return;
    const action = kind === 'none' ? 'lyrics.search' : kind === 'retry' ? 'lyrics.retryBtn' : '';
    el.dataset.kind = kind;
    $('statemsg').textContent = t({ idle: 'lyrics.noSong', loading: 'lyrics.loading', none: 'lyrics.none', retry: 'lyrics.retry' }[kind]);
    $('statebtn').hidden = !action;
    if (action) $('statebtn').textContent = t(action);
  }

  function clearLines() {
    stopFrames();
    view = null;
    shownData = null;
    box.hidden = true;
    box.textContent = '';
  }

  function render() {
    if (!on) return;
    if (onRender) onRender();
    const d = ly.data;
    const hasLines = !!d && d.status === 'ok' && d.lines.length > 0;
    if (!hasLines) {
      clearLines();
      $('tools').hidden = true;
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
      box.hidden = false;
      view = renderLines(box, d, { trans, onSeek: seekLine });
      view.tick(nowMs());
      view.center(false);
      recenter = false;
    }
    $('tools').hidden = false;
    $('offsetbox').hidden = !view.timed;
    $('offset').textContent = offsetText(d.offset || 0);
    $('trans').hidden = !hasTrans;
    $('trans').setAttribute('aria-pressed', String(!!ly.trans));
    const src = sourceName(d.source);
    $('src').textContent = src ? t('lyrics.sourceLabel', { name: src }) : '';
    kick();
  }

  $('statebtn').onclick = () => {
    if ($('state').dataset.kind === 'retry') reload();
    else openLyricSearch();
  };
  $('earlier').onclick = () => setOffset(OFFSET_STEP);
  $('later').onclick = () => setOffset(-OFFSET_STEP);
  $('trans').onclick = () => setTrans(!ly.trans).catch((e) => {
    console.error('lyrics_set_settings', e);
    toast(t('lyrics.saveError'), { icon: 'x', autoHide: 6000 });
  });
  $('search').onclick = openLyricSearch;
  $('stateicon').innerHTML = icon('lyrics');

  box.addEventListener('wheel', manualScroll, { passive: true });
  box.addEventListener('touchmove', manualScroll, { passive: true });
  box.addEventListener('keydown', (e) => { if (['PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown'].includes(e.key)) manualScroll(); });
  // dragging the scrollbar: the press lands on the container itself
  box.addEventListener('pointerdown', (e) => { pointerHeld = e.target === box; });
  window.addEventListener('pointerup', () => { pointerHeld = false; });
  window.addEventListener('pointercancel', () => { pointerHeld = false; });
  box.addEventListener('scroll', () => { if (pointerHeld) manualScroll(); }, { passive: true });
  new ResizeObserver(() => { if (on && view && gate.allowed(performance.now())) view.center(false); }).observe(box);

  subscribe(render);
  onClock(kick);
  document.addEventListener('visibilitychange', () => { if (document.hidden) stopFrames(); else kick(); });

  return {
    /** Call after the container is visible, so the first centring has its real size. */
    show() {
      if (on) return;
      on = true;
      release = holdView();
      render();
      if (view) view.center(false);
    },
    hide() {
      if (!on) return;
      on = false;
      stopFrames();
      clearTimeout(gateTimer);
      gate.clear();
      recenter = false;
      if (release) release();
      release = null;
      clearLines(); // rebuilt on show, so a hidden view holds no nodes for a long song
    },
    shown: () => on,
  };
}
