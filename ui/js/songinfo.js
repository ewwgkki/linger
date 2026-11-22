// "Song details" dialog, opened from the song's right-click menu.
import { t, lang } from './i18n.js';
import { fmtLabel, mmss, quality } from './util.js';

const $ = (id) => document.getElementById(id);

/** [[label key, text]] for a song; fields it does not have are left out. */
export function songFacts(s, path) {
  const f = [['detail.fTitle', s.t], ['detail.fArtist', s.a || t('song.unknownArtist')]];
  if (s.b) f.push(['detail.fAlbum', s.b]);
  if (s.n) f.push(['detail.fTrack', String(s.n)]);
  f.push(['detail.fFormat', fmtLabel(s)],
    ['detail.fQuality', `${t('q.' + quality(s))} · ${s.br} kbps`],
    ['detail.fDuration', mmss(s.s)],
    ['detail.fSize', `${(s.z / 1048576).toFixed(1)} MB`]);
  if (s.ad) f.push(['detail.fAdded', new Date(s.ad * 1000).toLocaleString(lang(), { year: 'numeric', month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })]);
  f.push(['detail.fPath', path]);
  return f;
}

let wired = false;
let copyPath = () => {};
let resetTimer = 0;

function wire() {
  wired = true;
  const d = $('songinfo'), btn = $('songinfocopy');
  $('songinfoclose').onclick = () => d.close();
  $('songinfox').onclick = () => d.close();
  btn.onclick = () => {
    clearTimeout(resetTimer);
    copyPath().then(() => { btn.textContent = t('detail.copied'); }, () => {
      const el = $('songinfopath');
      if (el) { const r = document.createRange(); r.selectNodeContents(el); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r); }
      btn.textContent = t('detail.selected');
    }).finally(() => { resetTimer = setTimeout(() => { btn.textContent = t('detail.copyPath'); }, 1500); });
  };
  const outside = (e) => { const r = d.getBoundingClientRect(); return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom; };
  let pressed = false;
  d.addEventListener('mousedown', (e) => { pressed = e.button === 0 && outside(e); });
  d.addEventListener('click', (e) => { if (pressed && outside(e)) d.close(); pressed = false; });
}

/** `copy(text)` returns a promise. */
export function openSongInfo(s, path, copy) {
  if (!wired) wire();
  copyPath = () => copy(path);
  clearTimeout(resetTimer);
  $('songinfocopy').textContent = t('detail.copyPath');
  const list = $('songinfolist');
  list.replaceChildren(...songFacts(s, path).flatMap(([k, v]) => {
    const dt = document.createElement('dt'), dd = document.createElement('dd');
    dt.textContent = t(k);
    dd.textContent = v;
    if (k === 'detail.fPath') dd.id = 'songinfopath';
    return [dt, dd];
  }));
  if (!$('songinfo').open) $('songinfo').showModal();
}
