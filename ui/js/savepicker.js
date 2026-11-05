// The heart: "save to…" favorites (id 0) and playlists. With no playlists it toggles favorites
// directly; otherwise a popover with one checkbox per target, applied by a single save_songs on "done".
import { t } from './i18n.js';
import { api } from './api.js';
import { esc, norm, icon } from './util.js';
import { pl, FAV, isSaved, createPlaylist, saveResultKey, errKey } from './playlists.js';
import { askName } from './dialogs.js';
import { toast } from './settings.js';

/** MembershipDto → [{ id, state }]: favorites first, then each listed playlist. */
export function pickerRows(m) {
  const st = (n) => (n > 0 && n >= m.total ? 'on' : n > 0 ? 'some' : 'off');
  return [{ id: FAV, state: st(m.fav || 0) }, ...(m.lists || []).map(([id, n]) => ({ id, state: st(n) }))];
}

/** Click on a row: half-ticked becomes ticked, then it alternates; it never goes back to half. */
export const cycle = (state) => (state === 'on' ? 'off' : 'on');

/** Two { id: state } maps → what save_songs needs; unchanged rows are not sent. */
export function diff(initial, current) {
  const add = [], remove = [];
  for (const [k, now] of Object.entries(current)) {
    const was = initial[k] || 'off';
    if (now === was) continue;
    if (now === 'on') add.push(+k);
    else if (now === 'off') remove.push(+k);
  }
  return { add, remove };
}

const $ = (id) => document.getElementById(id);
const showErr = (e) => toast(t(errKey(e)), { icon: 'x', autoHide: 6000 });

async function apply(ids, add, remove) {
  try {
    const r = await api.invoke('save_songs', { songIds: ids, add, remove });
    const [k, p] = saveResultKey(r || {});
    toast(t(k, p), { icon: 'check', autoHide: 3000 });
  } catch (e) { showErr(e); }
}

/** Open popover state, or null. */
let cur = null;

export const pickerOpen = () => !!cur;

/** Closes without saving. */
export function closePicker() {
  if (!cur) return;
  const { prevFocus } = cur;
  cur = null;
  $('savepicker').hidden = true;
  $('savepicker').innerHTML = '';
  document.removeEventListener('pointerdown', onOutside, true);
  document.removeEventListener('keydown', onKey, true);
  window.removeEventListener('resize', closePicker);
  if (prevFocus && prevFocus.isConnected) prevFocus.focus({ preventScroll: true });
}

function onOutside(e) {
  if (!cur || cur.busy) return;
  if (!$('savepicker').contains(e.target)) closePicker();
}

function onKey(e) {
  if (!cur || cur.busy || e.key !== 'Escape') return;
  e.preventDefault();
  e.stopImmediatePropagation();
  closePicker();
}

/** Places `el` next to `anchor` (an element or a { x, y } point), flipping when it would leave the window. */
export function place(el, anchor) {
  const r = anchor instanceof Element ? anchor.getBoundingClientRect() : { left: anchor.x, right: anchor.x, top: anchor.y, bottom: anchor.y };
  const w = el.offsetWidth, h = el.offsetHeight, m = 8;
  let x = anchor instanceof Element ? r.right - w : r.left;
  if (x + w > innerWidth - m) x = (anchor instanceof Element ? r.right : r.left) - w;
  x = Math.max(m, Math.min(x, innerWidth - w - m));
  let y = r.bottom + 4;
  if (y + h > innerHeight - m) y = r.top - h - 4;
  y = Math.max(m, Math.min(y, innerHeight - h - m));
  el.style.left = Math.round(x) + 'px';
  el.style.top = Math.round(y) + 'px';
}

const name = (id) => (id === FAV ? t('fav.title') : (pl.list.find((p) => p.id === id) || {}).name || '');

function rowHtml(id, state, hidden) {
  const checked = state === 'on' ? 'true' : state === 'some' ? 'mixed' : 'false';
  return `<button type="button" class="sp-row" role="menuitemcheckbox" aria-checked="${checked}" data-sp="${id}"${hidden ? ' hidden' : ''}>` +
    `<span class="sp-box" aria-hidden="true">${state === 'on' ? icon('check') : state === 'some' ? '<i></i>' : ''}</span>` +
    `${icon(id === FAV ? 'heart' : 'list')}<span class="sp-name">${esc(name(id))}</span></button>`;
}

function renderRows() {
  const q = norm(cur.filter.trim());
  $('sprows').innerHTML = cur.order.map((id) => rowHtml(id, cur.state[id], q && id !== FAV && !norm(name(id)).includes(q))).join('');
}

async function openPicker(ids, anchor) {
  closePicker();
  const token = {};
  cur = { token, ids, busy: false, filter: '', order: [], initial: {}, state: {}, prevFocus: document.activeElement };
  let m;
  try {
    m = await api.invoke('get_memberships', { songIds: ids });
  } catch (e) { cur = null; showErr(e); return; }
  if (!cur || cur.token !== token) return;
  const known = new Map(pickerRows(m).map((r) => [r.id, r.state]));
  cur.order = [FAV, ...pl.list.map((p) => p.id)];
  for (const id of cur.order) cur.initial[id] = cur.state[id] = known.get(id) || 'off';

  const el = $('savepicker');
  el.innerHTML = `<div class="sp-head">${esc(t('save.title'))}<span>${esc(t('meta.songs', { n: ids.length }))}</span></div>` +
    (pl.list.length > 8 ? `<input class="sp-filter" id="spfilter" type="search" autocomplete="off" spellcheck="false" placeholder="${esc(t('save.filter'))}" aria-label="${esc(t('save.filter'))}">` : '') +
    `<div class="sp-rows" id="sprows" role="menu"></div>` +
    `<div class="sp-foot"><button type="button" class="btn sp-new" id="spnew">${icon('new')}${esc(t('playlist.new'))}</button>` +
    `<span></span><button type="button" class="btn" id="spcancel">${esc(t('dlg.cancel'))}</button>` +
    `<button type="button" class="btn primary" id="spdone">${esc(t('save.done'))}</button></div>`;
  renderRows();
  el.hidden = false;
  place(el, anchor);
  document.addEventListener('pointerdown', onOutside, true);
  document.addEventListener('keydown', onKey, true);
  window.addEventListener('resize', closePicker);

  $('sprows').onclick = (e) => {
    const b = e.target.closest('[data-sp]');
    if (!b) return;
    const id = +b.dataset.sp;
    cur.state[id] = cycle(cur.state[id]);
    renderRows();
    $('sprows').querySelector(`[data-sp="${id}"]`)?.focus();
  };
  $('sprows').onkeydown = (e) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const rows = [...$('sprows').querySelectorAll('.sp-row:not([hidden])')];
    const i = rows.indexOf(document.activeElement);
    rows[(i + (e.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length]?.focus();
  };
  if ($('spfilter')) $('spfilter').oninput = (e) => { cur.filter = e.target.value; renderRows(); };
  $('spcancel').onclick = closePicker;
  $('spdone').onclick = () => {
    const { add, remove } = diff(cur.initial, cur.state);
    const songIds = cur.ids;
    closePicker();
    if (add.length || remove.length) apply(songIds, add, remove);
  };
  $('spnew').onclick = async () => {
    const c = cur;
    c.busy = true;
    let made = null;
    try {
      await askName({ title: t('playlist.newTitle'), ok: t('dlg.create'), run: async (n) => { made = await createPlaylist(n, []); } });
    } finally { c.busy = false; }
    if (cur !== c || !made) return;
    if (!c.order.includes(made.id)) c.order.push(made.id);
    c.initial[made.id] = 'off';
    c.state[made.id] = 'on';
    renderRows();
    place($('savepicker'), anchor);
    $('sprows').querySelector(`[data-sp="${made.id}"]`)?.focus();
  };
  ($('spfilter') || $('sprows').querySelector('.sp-row'))?.focus({ preventScroll: true });
}

/**
 * The heart. `ids`: songs (in the library) to save. `anchor`: the element or { x, y } point the popover
 * sits next to. With no playlists yet it toggles favorites directly.
 */
export async function saveSongs(ids, anchor, ctx) {
  ids = [...new Set(ids)].filter((id) => !ctx || !ctx.songsById || ctx.songsById.has(id));
  if (!ids.length) { toast(t('save.nothing'), { icon: 'check', autoHide: 3000 }); return; }
  if (!pl.list.length) {
    // no playlists: "saved" means "in favorites"
    const all = ids.every(isSaved);
    await apply(ids, all ? [] : [FAV], all ? [FAV] : []);
    return;
  }
  await openPicker(ids, anchor);
}
