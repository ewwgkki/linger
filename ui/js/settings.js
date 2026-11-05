// Settings dialog (music folders, scan, FFmpeg), scan progress and the toast.
import { t, lang } from './i18n.js';
import { api } from './api.js';
import { esc, icon } from './util.js';

const $ = (id) => document.getElementById(id);
/** With no scan event for this long, the scan thread is presumed dead and the UI is reset. */
const STALL_MS = 60000;
/** A scan the user did not start (startup, folder added) shows its toast only after this long. */
const QUIET_MS = 2500;

let ctx = null;
let reload = async () => {};

// ---------- toast ----------
let toastTimer = 0;
/** Bottom-right notice, as in the web version. `actions`: [{ label, run }]. */
export function toast(msg, { progress = null, actions = [], icon: ic = 'refresh', spin = false, autoHide = 0 } = {}) {
  clearTimeout(toastTimer);
  $('toast').hidden = false;
  $('toastmsg').textContent = msg;
  $('toasticon').innerHTML = `<use href="#i-${ic}"/>`;
  $('toasticon').parentElement.classList.toggle('spin', spin);
  $('toastprog').hidden = progress === null;
  if (progress !== null) $('toastprog').firstElementChild.style.width = progress + '%';
  $('toastacts').innerHTML = '';
  $('toastacts').hidden = !actions.length;
  for (const a of actions) {
    const b = document.createElement('button');
    b.type = 'button'; b.className = 'btn'; b.textContent = a.label; b.onclick = a.run;
    $('toastacts').append(b);
  }
  if (autoHide) toastTimer = setTimeout(hideToast, autoHide);
}
export function hideToast() { clearTimeout(toastTimer); $('toast').hidden = true; }

// ---------- settings dialog ----------
export function openSettings() {
  const d = $('settings');
  if (!d.open) d.showModal();
  renderRoots();
  scanInfo();
  ffmpegInfo();
}

function renderRoots() {
  const roots = ctx.lib.roots;
  $('rootlist').innerHTML = roots.length ? roots.map((r) => `<div class="prow${r.online ? '' : ' off'}">${icon(r.online ? 'folder' : 'cloud-off')}
      <span class="rp" title="${esc(r.path)}">${esc(r.path)}</span>
      <button class="rm" type="button" data-rm="${r.id}">${esc(t('settings.remove'))}</button>
      <span class="st ${r.online ? 'ok' : 'bad'}">${esc(t(r.online ? 'settings.online' : 'settings.offline', { n: r.count }))}</span>
    </div>`).join('') : `<p class="hint">${esc(t('settings.noRoots'))}</p>`;
}

function scanInfo() {
  const { scanned_at, songs } = ctx.lib;
  $('scaninfo').textContent = scanned_at
    ? t('scan.lastInfo', { time: new Date(scanned_at * 1000).toLocaleString(lang()), n: songs.length })
    : t('scan.never');
}

function ffmpegInfo() {
  $('ffmpeginfo').textContent = t(ctx.caps.ffmpeg ? 'settings.ffmpegFound' : 'settings.ffmpegMissing');
  $('ffmpeginfo').classList.toggle('warn', !ctx.caps.ffmpeg);
}

async function pickFolders() {
  const tauri = globalThis.__TAURI__;
  const opts = { directory: true, multiple: true };
  if (tauri && tauri.dialog) return tauri.dialog.open(opts);
  if (tauri) return tauri.core.invoke('plugin:dialog|open', { options: opts });
  // browser preview: no native dialog
  const p = prompt(t('settings.promptPath'));
  return p ? [p] : null;
}

async function addFolders() {
  let picked;
  try {
    picked = await pickFolders();
  } catch (e) {
    $('rootsmsg').textContent = t('settings.addError', { msg: errText(e) });
    return;
  }
  if (!picked || (Array.isArray(picked) && !picked.length)) return;
  const paths = Array.isArray(picked) ? picked : [picked];
  const had = new Set(ctx.lib.roots.map((r) => r.id));
  try {
    const roots = await api.invoke('add_roots', { paths });
    ctx.lib.roots = roots;
    ctx.sidebar();
    renderRoots();
    const added = roots.filter((r) => !had.has(r.id)).length;
    $('rootsmsg').textContent = added ? t('settings.added', { n: added }) : t('settings.alreadyAdded');
    if (added) startScan(true);
  } catch (e) {
    $('rootsmsg').textContent = t('settings.addError', { msg: errText(e) });
  }
}

async function removeRoot(id) {
  try {
    ctx.lib.roots = await api.invoke('remove_root', { id });
    $('rootsmsg').textContent = t('settings.removed');
    await reload();
  } catch (e) {
    $('rootsmsg').textContent = t('settings.addError', { msg: errText(e) });
  }
  renderRoots();
  scanInfo();
}

const errText = (e) => String((e && e.message) || e);

// ---------- scan ----------
const scan = { running: false, user: false, toast: false, started: 0, before: null, stall: 0 };

function scanUI(running) {
  $('refresh').disabled = running;
  $('refresh').classList.toggle('spin', running);
  $('scanbtn').disabled = running;
  $('scanprog').hidden = !running;
}

function armStall() {
  clearTimeout(scan.stall);
  scan.stall = setTimeout(() => {
    if (!scan.running) return;
    endScan();
    $('scanmsg').textContent = t('scan.stalled');
    toast(t('scan.stalled'), { icon: 'x' });
  }, STALL_MS);
}

function beginScan(user) {
  if (!scan.running) {
    scan.running = true;
    scan.started = performance.now();
    scan.toast = false;
    scan.before = ctx.loaded ? new Set(ctx.lib.songs.map((s) => s.id)) : null;
    $('skipbox').hidden = true;
    scanUI(true);
  }
  scan.user = scan.user || user;
  armStall();
}

function endScan() {
  clearTimeout(scan.stall);
  scan.running = false;
  scan.user = false;
  scanUI(false);
}

function progress(msg, pct) {
  $('scanmsg').textContent = msg;
  $('scanprog').firstElementChild.style.width = pct + '%';
  if (scan.user || scan.toast || performance.now() - scan.started > QUIET_MS) {
    scan.toast = true;
    toast(msg, { progress: pct, spin: true });
  }
}

/** Asks the backend for a scan (a request during a running scan queues one more pass). */
export function startScan(user = true) {
  if (!ctx.lib.roots.length) { openSettings(); return; }
  const fresh = !scan.running;
  beginScan(user);
  if (fresh) progress(t('scan.finding'), 0);
  else if (user) scan.toast = true; // already running: the next progress event shows the toast
  api.invoke('start_scan').catch((e) => {
    endScan();
    const msg = t('scan.error', { msg: errText(e) });
    $('scanmsg').textContent = msg;
    toast(msg, { icon: 'x', actions: [{ label: t('scan.openSettings'), run: () => { hideToast(); ctx.openSettings(); } }] });
  });
}

function onProgress({ done, total }) {
  beginScan(false);
  progress(t('scan.progress', { done, total }), total ? (done / total) * 100 : 100);
}

async function onFinished({ count, skipped = [] }) {
  const wasUser = scan.user, shown = scan.toast, before = scan.before;
  endScan();
  try { await reload(); } catch (e) { console.error('get_library', e); }
  scanInfo();
  renderRoots();
  let change = null, changed = false;
  if (before) {
    const added = ctx.lib.songs.filter((s) => !before.has(s.id)).length;
    const removed = before.size - (ctx.lib.songs.length - added);
    changed = added > 0 || removed > 0;
    change = changed
      ? [added ? t('scan.added', { n: added }) : '', removed > 0 ? t('scan.removed', { n: removed }) : ''].filter(Boolean).join(t('list.sep'))
      : t('scan.noChange');
  }
  const msg = change ? t('scan.done', { n: count, change }) : t('scan.doneCount', { n: count });
  $('scanmsg').textContent = msg;
  $('scanprog').firstElementChild.style.width = '100%';
  if (skipped.length) {
    $('skipbox').hidden = false;
    $('skipsum').textContent = t('scan.skippedTitle', { n: skipped.length });
    $('skips').innerHTML = skipped.map((x) => `<li>${esc(x.path)} &mdash; ${esc(t(x.why))}</li>`).join('');
  }
  if (!(wasUser || shown || changed)) return;
  if (skipped.length) {
    toast(t('scan.withSkipped', { msg, n: skipped.length }), {
      icon: 'check',
      actions: [{ label: t('scan.view'), run: () => { hideToast(); ctx.openSettings(); $('skipbox').open = true; } }],
    });
  } else toast(msg, { icon: 'check', autoHide: 5000 });
}

export function initSettings(c, { loadLibrary }) {
  ctx = c;
  reload = loadLibrary;
  $('toastx').onclick = hideToast;
  $('addfolder').onclick = addFolders;
  $('scanbtn').onclick = () => startScan(true);
  $('refresh').onclick = () => startScan(true);
  $('rootlist').addEventListener('click', (e) => {
    const b = e.target.closest('[data-rm]');
    if (b) removeRoot(+b.dataset.rm);
  });
  // sent by the scan thread right after probing the folders, before any progress
  api.listen('library://roots', (roots) => {
    beginScan(false);
    ctx.lib.roots = roots;
    ctx.sidebar();
    if ($('settings').open) renderRoots();
  });
  api.listen('scan://progress', onProgress);
  api.listen('scan://finished', onFinished);
}
