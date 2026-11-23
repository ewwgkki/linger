// "Check for updates" card in Settings > About. Nothing installs without a click.
import { t, onLanguage } from './i18n.js';
import { api } from './api.js';
import { esc } from './util.js';

const $ = (id) => document.getElementById(id);
const AUTO_CHECK_DELAY = 4000;

export const initialUpdate = () => ({ phase: 'idle', version: '', notes: '', pct: 0, error: '', showNotes: false });

/** Pure state changes, so every path through the update flow can be tested. */
export function nextUpdate(s, ev) {
  switch (ev.type) {
    case 'check': return { ...s, phase: 'checking', error: '' };
    case 'found':
      if (!ev.info) return { ...s, phase: ev.manual ? 'latest' : 'idle', version: '', notes: '', showNotes: false };
      return { ...s, phase: 'available', version: ev.info.version, notes: ev.info.notes || '', error: '', pct: 0 };
    case 'checkFailed': return { ...s, phase: ev.manual ? 'error' : 'idle', error: ev.manual ? ev.error : '' };
    case 'download': return { ...s, phase: 'downloading', pct: 0, error: '' };
    case 'progress': return s.phase === 'downloading' ? { ...s, pct: ev.pct } : s;
    case 'ready': return s.phase === 'downloading' ? { ...s, phase: 'ready', pct: 100 } : s;
    case 'failed': return s.phase === 'downloading' ? { ...s, phase: 'error', error: 'update.downloadFailed' } : s;
    case 'cancel':
    case 'later': return { ...s, phase: 'available', pct: 0 };
    case 'install': return { ...s, phase: 'installing' };
    case 'installFailed': return { ...s, phase: 'ready', error: ev.error };
    case 'notes': return { ...s, showNotes: !s.showNotes };
    default: return s;
  }
}

export const hasUpdate = (s) => ['available', 'downloading', 'ready', 'installing'].includes(s.phase);

let state = initialUpdate();
const send = (ev) => { state = nextUpdate(state, ev); paint(); };
const errKey = (e) => {
  const k = String((e && e.message) || e);
  return k.startsWith('update.') ? k : 'update.checkFailed';
};

const btn = (act, label, cls = '') => `<button type="button" class="btn ${cls}" data-upd="${act}">${esc(t(label))}</button>`;

function cardHtml(s) {
  const v = { version: s.version };
  const err = s.error ? `<p class="set-sub set-bad" role="alert">${esc(t(s.error))}</p>` : '';
  switch (s.phase) {
    case 'checking':
      return `<div class="upd-top"><b>${esc(t('update.checking'))}</b></div>`;
    case 'available':
      return `<div class="upd-top"><div><b>${esc(t('update.available', v))}</b>${s.notes ? `<button type="button" class="upd-link" data-upd="notes" aria-expanded="${s.showNotes}">${esc(t(s.showNotes ? 'update.hideNotes' : 'update.showNotes'))}</button>` : ''}</div>${btn('download', 'update.download', 'primary')}</div>${s.showNotes ? `<p class="upd-notes">${esc(s.notes)}</p>` : ''}${err}`;
    case 'downloading':
      return `<div class="upd-top"><b>${esc(t('update.downloading', v))}</b><span class="set-sub">${s.pct}%</span></div>
        <div class="progress" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${s.pct}"><i style="width:${s.pct}%"></i></div>
        <div class="upd-acts">${btn('cancel', 'dlg.cancel')}</div>`;
    case 'ready':
      return `<div class="upd-top"><div><b>${esc(t('update.ready', v))}</b><p class="set-sub">${esc(t('update.readyHint'))}</p></div><div class="upd-acts">${btn('later', 'update.later')}${btn('install', 'update.install', 'primary')}</div></div>${err}`;
    case 'installing':
      return `<div class="upd-top"><b>${esc(t('update.installing'))}</b></div>`;
    case 'latest':
      return `<div class="upd-top"><b class="set-good">${esc(t('update.latest'))}</b>${btn('check', 'update.check')}</div>`;
    case 'error':
      return `<div class="upd-top"><b class="set-bad">${esc(t(s.error))}</b>${btn('check', 'update.retry')}</div>`;
    default:
      return `<div class="upd-top"><b>${esc(t('update.title'))}</b>${btn('check', 'update.check')}</div>`;
  }
}

function paint() {
  const card = $('updcard');
  const hadFocus = card.contains(document.activeElement) ? document.activeElement.dataset.upd : null;
  card.innerHTML = cardHtml(state);
  card.classList.toggle('on', hasUpdate(state));
  if (hadFocus) card.querySelector(`[data-upd="${hadFocus}"]`)?.focus();
  const dot = state.phase === 'available' || state.phase === 'ready';
  $('setdot').hidden = !dot;
  $('topdot').hidden = !dot;
}

async function check(manual) {
  if (state.phase === 'downloading' || state.phase === 'installing') return;
  if (manual) send({ type: 'check' });
  try {
    const info = await api.invoke('update_check', { manual });
    if (info || manual) send({ type: 'found', info, manual });
  } catch (e) {
    console.error('update_check', e);
    send({ type: 'checkFailed', manual, error: errKey(e) });
  }
}

async function act(name) {
  if (name === 'check') check(true);
  else if (name === 'notes' || name === 'later') send({ type: name });
  else if (name === 'download') {
    send({ type: 'download' });
    try { await api.invoke('update_download'); } catch (e) { console.error('update_download', e); send({ type: 'failed' }); }
  } else if (name === 'cancel') {
    send({ type: 'cancel' });
    api.invoke('update_cancel').catch((e) => console.error('update_cancel', e));
  } else if (name === 'install') {
    send({ type: 'install' });
    try { await api.invoke('update_install'); } catch (e) { console.error('update_install', e); send({ type: 'installFailed', error: errKey(e) }); }
  }
}

function paintVersion(version) {
  $('setversion').textContent = t('update.version', { version });
}

export async function initUpdate() {
  let version = '';
  paint();
  onLanguage(() => { paint(); if (version) paintVersion(version); });
  $('updcard').addEventListener('click', (e) => {
    const b = e.target.closest('[data-upd]');
    if (b) act(b.dataset.upd);
  });
  api.listen('update://progress', ({ pct }) => send({ type: 'progress', pct }));
  api.listen('update://ready', () => send({ type: 'ready' }));
  api.listen('update://failed', () => send({ type: 'failed' }));
  $('updautosw').onchange = (e) => {
    const on = e.target.checked;
    api.invoke('update_set_auto', { enabled: on }).catch((err) => {
      console.error('update_set_auto', err);
      e.target.checked = !on;
    });
  };
  try {
    const s = await api.invoke('update_settings');
    version = s.version;
    paintVersion(version);
    $('updautosw').checked = s.auto;
    if (s.auto) setTimeout(() => check(false), AUTO_CHECK_DELAY);
  } catch (e) {
    console.error('update_settings', e);
  }
}
