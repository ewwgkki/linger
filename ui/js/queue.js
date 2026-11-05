// Play queue panel: a drawer on the right with the playing song, the user queue ("up") and the
// rest of the play source ("ctx"). The backend owns the queue; this module mirrors
// `player_queue` while the panel is open and sends edits (optimistically re-rendered).
import { t } from './i18n.js';
import { api } from './api.js';
import { esc, mmss, icon } from './util.js';
import { art, artistName, gripHtml } from './views/songs.js';
import { attachReorder } from './reorder.js';
import { openQueueMenu, menuOpen, closeMenus } from './menu.js';
import { pickerOpen } from './savepicker.js';
import { sourceLabel } from './player.js';
import { errKey } from './playlists.js';
import { toast } from './settings.js';

const $ = (id) => document.getElementById(id);
/** Row height of the panel's lists (matches `.q-row` in app.css). */
const ROW_H = 56;

let ctx = null;
let isOpen = false;
/** Last `player_queue` result. */
let q = { now: null, up: [], ctx: [], ctx_total: 0 };
let seq = 0, timer = 0, lastId;
/**
 * A drag is in progress: fetched data waits until it ends. `stale`: the queue changed during the
 * drag, so the dragged indices may be out of date and the drop is dropped (re-fetched instead).
 */
let dragging = false, stale = false;

const showErr = (e) => toast(t(errKey(e)), { icon: 'x', autoHide: 6000 });

function rowHtml(id, part, i) {
  const s = ctx.songsById.get(id);
  const now = part === 'now';
  const title = s ? s.t : t('queue.unknownSong');
  const cls = `song q-row${now ? ' now' : ''}${s && !s.av ? ' off' : ''}${s ? '' : ' missing'}`;
  return `<div class="${cls}" data-id="${esc(id)}" data-part="${part}" data-ri="${i}" tabindex="0" role="listitem">
    ${now ? '<span></span>' : gripHtml()}${art(s, 'note')}
    <div class="s-main"><div class="s-l1">${now ? '<span class="eq"><i></i><i></i><i></i></span>' : ''}<span class="t">${esc(title)}</span></div>
    <div class="s-l2">${s ? esc([artistName(s.a), s.b].filter(Boolean).join(' · ')) : ''}</div></div>
    <span class="dur">${s ? mmss(s.s) : ''}</span>${now ? '<span></span>'
    : `<button class="rbtn q-rm" type="button" data-rm aria-label="${esc(t('queue.remove'))}" title="${esc(t('queue.remove'))}">${icon('x')}</button>`}
  </div>`;
}

function render() {
  const { now, up, ctx: rest, ctx_total: total } = q;
  $('qnowsec').hidden = !now;
  $('qnow').innerHTML = now ? rowHtml(now, 'now', 0) : '';
  $('qupsec').hidden = !up.length;
  $('qup').innerHTML = up.map((id, i) => rowHtml(id, 'up', i)).join('');
  $('qctxsec').hidden = !rest.length;
  const label = sourceLabel();
  $('qctxh').textContent = label ? t('queue.from', { name: label }) : t('queue.next');
  $('qctx').innerHTML = rest.map((id, i) => rowHtml(id, 'ctx', i)).join('');
  const more = Math.max(0, (total || 0) - rest.length);
  $('qmore').hidden = !more;
  $('qmore').textContent = more ? t('queue.more', { n: more }) : '';
  $('qempty').hidden = !!(now || up.length || rest.length);
}

/** Re-fetches the queue (coalesced; only while the panel is open). */
function refresh(delay = 30) {
  if (!isOpen) return;
  clearTimeout(timer);
  timer = setTimeout(() => {
    const my = ++seq;
    api.invoke('player_queue').then((d) => {
      if (my !== seq || !isOpen) return;
      if (dragging) { stale = true; return; }
      stale = false;
      q = { now: d.now || null, up: d.up || [], ctx: d.ctx || [], ctx_total: d.ctx_total || 0 };
      render();
    }, (e) => console.error('player_queue', e));
  }, delay);
}

/**
 * Sends an edit; on failure re-fetches the real queue. `queue.badIndex` means the queue changed
 * under the edit (e.g. the song advanced), so only the re-fetch is needed, no error toast.
 */
function send(cmd, args) {
  seq++; // a fetch already in flight is older than this edit
  return api.invoke(cmd, args).catch((e) => { if (errKey(e) !== 'queue.badIndex') showErr(e); refresh(0); });
}

const list = (part) => (part === 'up' ? q.up : q.ctx);

// Edits name the song expected at the index; the backend refuses if the queue moved on meanwhile.
function move(part, from, to) {
  const l = list(part);
  const id = l[from];
  if (id === undefined) return;
  l.splice(to, 0, ...l.splice(from, 1));
  render();
  send('player_queue_move', { part, from, to, id });
}

function remove(part, index) {
  const id = list(part)[index];
  if (id === undefined) return;
  list(part).splice(index, 1);
  if (part === 'ctx') q.ctx_total = Math.max(0, q.ctx_total - 1);
  render();
  send('player_queue_remove', { part, index, id });
}

function jump(part, index) {
  const id = list(part)[index];
  if (id !== undefined) send('player_queue_jump', { part, index, id });
}

function clearUp() {
  q.up = [];
  render();
  send('player_queue_clear');
}

export function openQueue() {
  if (isOpen) return;
  isOpen = true;
  $('queue').hidden = false;
  $('queuebtn').setAttribute('aria-pressed', 'true');
  document.body.classList.add('queue-open');
  refresh(0);
  $('queueclose').focus({ preventScroll: true });
}

export function closeQueue() {
  if (!isOpen) return;
  isOpen = false;
  clearTimeout(timer);
  seq++;
  const had = $('queue').contains(document.activeElement);
  $('queue').hidden = true;
  $('queuebtn').setAttribute('aria-pressed', 'false');
  document.body.classList.remove('queue-open');
  if (had) $('queuebtn').focus({ preventScroll: true });
}

export const toggleQueue = () => (isOpen ? closeQueue() : openQueue());
export const queueOpen = () => isOpen;

const rowOf = (el) => {
  const r = el.closest('.q-row');
  return r && r.dataset.part !== 'now' ? { r, part: r.dataset.part, index: +r.dataset.ri } : null;
};

export function initQueue(c) {
  ctx = c;
  $('queueclose').onclick = closeQueue;
  const body = $('qbody');
  for (const part of ['up', 'ctx']) {
    attachReorder($('q' + part), {
      rowH: ROW_H,
      handle: '[data-grip]',
      count: () => list(part).length,
      scroller: body,
      onStart() { dragging = true; stale = false; },
      onEnd() {
        dragging = false;
        if (stale) refresh(0);
      },
      onMove: (from, to) => { if (!stale) move(part, from, to); },
    });
  }
  $('qclear').onclick = clearUp;
  body.addEventListener('scroll', () => { if (menuOpen()) closeMenus(); }, { passive: true });
  body.addEventListener('click', (e) => {
    const b = e.target.closest('[data-rm]');
    const x = b && rowOf(b);
    if (x) remove(x.part, x.index);
  });
  body.addEventListener('dblclick', (e) => {
    if (e.target.closest('button')) return;
    const x = rowOf(e.target);
    if (x) jump(x.part, x.index);
  });
  body.addEventListener('keydown', (e) => {
    const x = e.key === 'Enter' && e.target.classList.contains('q-row') ? rowOf(e.target) : null;
    if (x) { e.preventDefault(); jump(x.part, x.index); }
  });
  body.addEventListener('contextmenu', (e) => {
    const r = e.target.closest('.q-row');
    if (!r) return;
    e.preventDefault();
    let { clientX: px, clientY: py } = e;
    if (!px && !py) { const b = r.getBoundingClientRect(); px = b.left + 60; py = b.bottom - 8; }
    const x = rowOf(r);
    openQueueMenu(px, py, r.dataset.id, ctx, x ? { onJump: () => jump(x.part, x.index), onRemove: () => remove(x.part, x.index) } : {});
  });

  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const typing = /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName) || document.activeElement.isContentEditable;
    const busy = typing || !!document.querySelector('dialog[open]') || menuOpen() || pickerOpen();
    if ((e.key === 'q' || e.key === 'Q') && !busy && !e.repeat) { e.preventDefault(); toggleQueue(); }
    else if (e.key === 'Escape' && isOpen && !busy) closeQueue();
  });

  api.listen('player://queue', () => refresh());
  api.listen('player://state', (st) => {
    if (st.id === lastId) return;
    lastId = st.id;
    refresh();
  });
}
