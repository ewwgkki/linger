// Settings dialog (General, Library, About), scan progress and the toast.
import { t, lang, onLanguage, setLanguage, LANGS, LANG_NAMES } from './i18n.js';
import { api } from './api.js';
import { SKINS, currentSkin, setSkin } from './skin.js';
import { rootLabel } from './views/songs.js';
import { esc, icon, applyOnline } from './util.js';
import { missingText, summaryKey } from './importview.js';
import { ly, setNetease } from './lyrics.js';

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
export function openSettings(tab) {
  const d = $('settings');
  const first = !d.open;
  if (first) d.showModal();
  if (tab || first) setTab(tab || (ctx.lib.roots.length ? 'general' : 'library'));
  showImportSheet(false);
  rootMenu = null;
  renderRoots();
  scanInfo();
  ffmpegInfo();
  $('lyneteasesw').checked = ly.netease;
  api.invoke('window_settings').then((s) => { $('closesel').value = s.close_to_tray ? 'tray' : 'quit'; })
    .catch((e) => console.error('window_settings', e));
}

const SET_TABS = ['general', 'library', 'about'];
function setTab(name) {
  for (const k of SET_TABS) {
    const on = k === name;
    const tab = $(`settab-${k}`);
    tab.setAttribute('aria-selected', String(on));
    tab.tabIndex = on ? 0 : -1;
    $(`setpane-${k}`).hidden = !on;
  }
  rootMenu = null;
  renderRoots();
  showImportSheet(false);
  document.querySelector('.set-main').scrollTop = 0;
}

function showImportSheet(on) {
  $('importsheet').hidden = !on;
  if (on) {
    document.querySelector('.set-main').scrollTop = 0;
    $('importbtn').focus({ preventScroll: true });
  }
}

/** Opens Settings on the Library page with the Spotify import steps showing. */
export function openImport() {
  openSettings('library');
  showImportSheet(true);
}

let rootMenu = null;
function renderRoots() {
  const roots = ctx.lib.roots;
  $('rootlist').innerHTML = roots.length ? roots.map((r) => {
    const count = t(r.online ? 'settings.folderCount' : 'settings.folderOffline', { n: r.count });
    const name = rootLabel(r);
    const menu = rootMenu === r.id ? `<div class="set-pop" role="menu">
        <button type="button" role="menuitem" data-root-act="refresh" data-id="${r.id}">${esc(t('menu.refreshFolder'))}</button>
        <button type="button" role="menuitem" data-root-act="open" data-id="${r.id}">${esc(t('settings.openFolder'))}</button>
        <button type="button" role="menuitem" class="danger" data-root-act="remove" data-id="${r.id}">${esc(t('settings.remove'))}</button>
      </div>` : '';
    return `<div class="set-folder${r.online ? '' : ' off'}" title="${esc(r.path)}">${icon(r.online ? 'folder' : 'cloud-off')}
      <span class="nm">${esc(name)}</span><span class="ct">${esc(count)}</span>
      <button class="set-more" type="button" data-root-more="${r.id}" aria-haspopup="menu" aria-expanded="${rootMenu === r.id}" aria-label="${esc(t('settings.folderMore', { name }))}">${icon('more')}</button>${menu}
    </div>`;
  }).join('') : `<p class="set-sub">${esc(t('settings.noRoots'))}</p>`;
}

function rootAction(act, id) {
  rootMenu = null;
  renderRoots();
  if (act === 'refresh') startScan(true, id);
  else if (act === 'remove') removeRoot(id);
  else if (act === 'open') {
    api.invoke('open_root', { id }).catch((e) => {
      console.error('open_root', e);
      toast(t('settings.openFailed'), { icon: 'x', autoHide: 5000 });
    });
  }
}

function scanInfo() {
  const { scanned_at, songs } = ctx.lib;
  $('scaninfo').textContent = scanned_at
    ? t('scan.lastInfo', { time: new Date(scanned_at * 1000).toLocaleString(lang()), n: songs.length })
    : t('scan.never');
}

let lastSkipped = [];
function paintSkips() {
  $('skipsum').textContent = t('scan.skippedTitle', { n: lastSkipped.length });
  $('skips').innerHTML = lastSkipped.map((x) => `<li>${esc(x.path)} &mdash; ${esc(t(x.why))}</li>`).join('');
}

function paintLanguages() {
  $('langsel').innerHTML = LANGS.map((l) => `<option value="${l}" lang="${l}">${esc(LANG_NAMES[l])}</option>`).join('');
  $('langsel').value = lang();
}

function paintSkins() {
  const now = currentSkin();
  $('skinseg').innerHTML = SKINS.map((s) => `<button type="button" data-skin-pick="${s}" aria-pressed="${s === now}">${esc(t(`settings.skin.${s}`))}</button>`).join('');
}

function languageChanged() {
  paintLanguages();
  paintSkins();
  renderRoots();
  scanInfo();
  ffmpegInfo();
  $('rootsmsg').textContent = '';
  $('importmsg').textContent = '';
  if (lastSkipped.length) paintSkips();
  if (!scan.running) { // while scanning, the next progress event rewrites these
    $('scanmsg').textContent = '';
    hideToast();
  }
}

function ffmpegInfo() {
  $('ffmpegrow').hidden = !!ctx.caps.ffmpeg;
  $('ffmpeginfo').textContent = ctx.caps.ffmpeg ? '' : t('settings.ffmpegMissing');
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

// ---------- Spotify import ----------
async function pickExports() {
  const tauri = globalThis.__TAURI__;
  const opts = { multiple: true, directory: false, filters: [{ name: t('import.pickFilter'), extensions: ['csv', 'zip', 'json'] }] };
  if (tauri && tauri.dialog) return tauri.dialog.open(opts);
  if (tauri) return tauri.core.invoke('plugin:dialog|open', { options: opts });
  const p = prompt(t('import.promptPath'));
  return p ? [p] : null;
}

async function runImport() {
  let picked;
  try {
    picked = await pickExports();
  } catch (e) {
    $('importmsg').textContent = t('import.error', { msg: errText(e) });
    return;
  }
  if (!picked || (Array.isArray(picked) && !picked.length)) return;
  const paths = Array.isArray(picked) ? picked : [picked];
  $('importmsg').textContent = '';
  $('importbtn').disabled = true;
  toast(t('import.running'), { spin: true });
  try {
    const report = await api.invoke('import_spotify', { paths });
    hideToast();
    showReport(report);
  } catch (e) {
    toast(t('import.error', { msg: t(errText(e)) }), { icon: 'x', autoHide: 6000 });
    $('importmsg').textContent = t('import.error', { msg: t(errText(e)) });
  } finally {
    $('importbtn').disabled = false;
  }
}

const fileName = (p) => String(p).split(/[\\/]/).pop();

export function showReport(report) {
  const [key, params] = summaryKey(report);
  $('importsum').textContent = report.playlists.length ? t(key, params) : t('import.empty');
  $('importlists').innerHTML = report.playlists.map((p) => {
    const name = p.id != null
      ? `<button type="button" class="imp-name" data-pl="${p.id}">${esc(p.name)}</button>`
      : `<span class="imp-name off">${esc(p.name)}</span>`;
    return `<div class="imp-row">${name}<span class="imp-n">${p.matched}/${p.total}</span>${p.id == null ? `<span class="imp-tag">${esc(t('import.notCreated'))}</span>` : ''}</div>`;
  }).join('');
  $('importfailbox').hidden = !report.failed.length;
  $('importfails').innerHTML = report.failed.map((f) => `<li><b>${esc(fileName(f.source))}</b> &mdash; ${esc(t(f.reason))}</li>`).join('');
  const text = missingText(report);
  $('importmissing').value = text;
  $('importmissing').hidden = true;
  $('importcopy').hidden = !text;
  $('importcopy').textContent = t('import.copyMissing');
  const d = $('importdlg');
  if (!d.open) d.showModal();
}

function copyMissing() {
  const btn = $('importcopy'), box = $('importmissing');
  const reset = () => setTimeout(() => { btn.textContent = t('import.copyMissing'); }, 1500);
  const fallback = () => {
    box.hidden = false; box.focus(); box.select();
    btn.textContent = t('import.selected'); reset();
  };
  const done = () => { btn.textContent = t('import.copied'); reset(); };
  if (navigator.clipboard && window.isSecureContext) navigator.clipboard.writeText(box.value).then(done).catch(fallback);
  else fallback();
}

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
    lastSkipped = [];
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
export function startScan(user = true, root = null) {
  if (!ctx.lib.roots.length) { openSettings(); return; }
  const fresh = !scan.running;
  beginScan(user);
  if (fresh) progress(t('scan.finding'), 0);
  else if (user) scan.toast = true; // already running: the next progress event shows the toast
  api.invoke('start_scan', { root }).catch((e) => {
    endScan();
    const msg = t('scan.error', { msg: errText(e) });
    $('scanmsg').textContent = msg;
    toast(msg, { icon: 'x', actions: [{ label: t('scan.openSettings'), run: () => { hideToast(); openSettings('library'); } }] });
  });
}

function onProgress({ done, total }) {
  beginScan(false);
  progress(t('scan.progress', { done, total }), total ? (done / total) * 100 : 100);
}

async function onFinished({ count, skipped = [], renamed = 0, root = null }) {
  const wasUser = scan.user, shown = scan.toast, before = scan.before;
  endScan();
  try { await reload(); } catch (e) { console.error('get_library', e); }
  scanInfo();
  renderRoots();
  let change = null, changed = false;
  if (before) {
    const newIds = ctx.lib.songs.filter((s) => !before.has(s.id)).length;
    const gone = before.size - (ctx.lib.songs.length - newIds);
    const moved = Math.min(renamed, newIds, Math.max(gone, 0));
    const added = newIds - moved, removed = gone - moved;
    changed = added > 0 || removed > 0 || moved > 0;
    change = changed
      ? [added ? t('scan.added', { n: added }) : '', removed > 0 ? t('scan.removed', { n: removed }) : '', moved ? t('scan.renamed', { n: moved }) : ''].filter(Boolean).join(t('list.sep'))
      : t('scan.noChange');
  }
  const folder = root != null ? ctx.lib.roots.find((r) => r.id === root) : null;
  const msg = folder
    ? t('scan.doneFolder', { name: rootLabel(folder), n: ctx.lib.songs.filter((s) => s.r === root).length, change: change || t('scan.noChange') })
    : change ? t('scan.done', { n: count, change }) : t('scan.doneCount', { n: count });
  $('scanmsg').textContent = msg;
  $('scanprog').firstElementChild.style.width = '100%';
  if (skipped.length) {
    $('skipbox').hidden = false;
    lastSkipped = skipped;
    paintSkips();
  }
  if (!(wasUser || shown || changed)) return;
  if (skipped.length) {
    toast(t('scan.withSkipped', { msg, n: skipped.length }), {
      icon: 'check',
      actions: [{ label: t('scan.view'), run: () => { hideToast(); openSettings('library'); $('skipbox').open = true; } }],
    });
  } else toast(msg, { icon: 'check', autoHide: 5000 });
}

/** New folder list / online flags: grey out (or re-enable) their songs right away. */
function rootsChanged(roots) {
  ctx.lib.roots = roots;
  const avChanged = applyOnline(ctx.lib.songs, roots);
  ctx.sidebar();
  if (avChanged && ctx.loaded) ctx.render();
  if ($('settings').open) renderRoots();
}

export function initSettings(c, { loadLibrary }) {
  ctx = c;
  reload = loadLibrary;
  $('toastx').onclick = hideToast;
  paintLanguages();
  paintSkins();
  onLanguage(languageChanged);
  $('skinseg').addEventListener('click', (e) => {
    const b = e.target.closest('[data-skin-pick]');
    if (!b) return;
    setSkin(b.dataset.skinPick);
    paintSkins();
  });
  $('langsel').onchange = (e) => {
    setLanguage(e.target.value).catch(() => {
      e.target.value = lang();
      toast(t('settings.langError'), { icon: 'x', autoHide: 6000 });
    });
  };
  const nav = document.querySelector('.set-nav');
  nav.addEventListener('click', (e) => {
    const b = e.target.closest('[data-set-tab]');
    if (b) setTab(b.dataset.setTab);
  });
  nav.addEventListener('keydown', (e) => {
    const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[e.key];
    const b = e.target.closest('[data-set-tab]');
    if (!step || !b) return;
    e.preventDefault();
    const next = SET_TABS[(SET_TABS.indexOf(b.dataset.setTab) + step + SET_TABS.length) % SET_TABS.length];
    setTab(next);
    $(`settab-${next}`).focus();
  });
  document.querySelector('.set-links').addEventListener('click', (e) => {
    const b = e.target.closest('[data-link]');
    if (!b) return;
    api.invoke('open_link', { link: b.dataset.link }).catch((err) => {
      console.error('open_link', err);
      toast(t('settings.openFailed'), { icon: 'x', autoHide: 5000 });
    });
  });
  $('importopen').onclick = () => showImportSheet(true);
  $('importback').onclick = () => { showImportSheet(false); $('importopen').focus(); };
  // A modal blocks the whole page: clicking the dimmed area outside it closes it.
  const box = $('settings');
  const outside = (e) => {
    const r = box.getBoundingClientRect();
    return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
  };
  let pressedOutside = false;
  box.addEventListener('cancel', (e) => {
    if (rootMenu != null) { e.preventDefault(); rootMenu = null; renderRoots(); }
    else if (!$('importsheet').hidden) { e.preventDefault(); showImportSheet(false); $('importopen').focus(); }
  });
  box.addEventListener('mousedown', (e) => {
    pressedOutside = e.button === 0 && outside(e);
    if (rootMenu != null && !e.target.closest('.set-pop, [data-root-more]')) { rootMenu = null; renderRoots(); }
  });
  box.addEventListener('click', (e) => {
    if (pressedOutside && outside(e)) box.close();
    pressedOutside = false;
  });
  $('addfolder').onclick = addFolders;
  $('scanbtn').onclick = () => startScan(true);
  $('importbtn').onclick = runImport;
  $('importcopy').onclick = copyMissing;
  $('lyneteasesw').onchange = (e) => setNetease(e.target.checked).catch((err) => {
    console.error('lyrics_set_settings', err);
    e.target.checked = ly.netease;
    toast(t('lyrics.saveError'), { icon: 'x', autoHide: 6000 });
  });
  $('closesel').onchange = (e) => {
    const on = e.target.value === 'tray';
    api.invoke('window_set_close_to_tray', { enabled: on }).catch((err) => {
      console.error('window_set_close_to_tray', err);
      e.target.value = on ? 'quit' : 'tray';
      toast(t('settings.closeToTrayError'), { icon: 'x', autoHide: 6000 });
    });
  };
  $('importclose').onclick = () => $('importdlg').close();
  $('importx').onclick = () => $('importdlg').close();
  $('importlists').addEventListener('click', (e) => {
    const b = e.target.closest('[data-pl]');
    if (!b) return;
    $('importdlg').close();
    $('settings').close();
    ctx.openPlaylist(+b.dataset.pl);
  });
  $('refresh').onclick = () => startScan(true);
  $('rootlist').addEventListener('click', (e) => {
    const more = e.target.closest('[data-root-more]');
    if (more) {
      const id = +more.dataset.rootMore;
      rootMenu = rootMenu === id ? null : id;
      renderRoots();
      return;
    }
    const b = e.target.closest('[data-root-act]');
    if (b) rootAction(b.dataset.rootAct, +b.dataset.id);
  });
  // sent by the scan thread right after probing the folders, before any progress
  api.listen('library://roots', (roots) => {
    beginScan(false);
    rootsChanged(roots);
  });
  // sent by the player when a failing song's folder turned out to be unreachable (no scan)
  api.listen('library://online', rootsChanged);
  api.listen('scan://progress', onProgress);
  api.listen('scan://finished', onFinished);
}
