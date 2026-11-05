// Small modal dialogs: ask for a name, confirm an action.
import { t } from './i18n.js';
import { errKey } from './playlists.js';

const $ = (id) => document.getElementById(id);

/**
 * Asks for a name. Resolves to the trimmed name, or null when cancelled. With `run(name)` the dialog
 * stays open while it runs; if it rejects, the error (an i18n key, or a raw message) is shown in the
 * dialog and the user can try again.
 */
export function askName({ title, value = '', ok, run = null }) {
  const d = $('namedlg'), input = $('nameinput'), err = $('nameerr'), okBtn = $('nameok');
  $('nametitle').textContent = title;
  okBtn.textContent = ok || t('dlg.ok');
  input.value = value;
  err.textContent = '';
  err.hidden = true;
  okBtn.disabled = false;
  return new Promise((resolve) => {
    let busy = false;
    const fail = (msg) => { err.textContent = msg; err.hidden = false; input.focus(); };
    const done = (v) => {
      d.removeEventListener('submit', submit);
      d.removeEventListener('close', closed);
      $('namecancel').onclick = null;
      input.oninput = null;
      if (d.open) d.close();
      resolve(v);
    };
    async function submit(e) {
      e.preventDefault();
      if (busy) return;
      const name = input.value.trim();
      if (!name) { fail(t('playlist.nameEmpty')); return; }
      if (!run) { done(name); return; }
      busy = true; okBtn.disabled = true;
      try {
        await run(name);
        done(name);
      } catch (x) {
        fail(t(errKey(x)));
      } finally {
        busy = false; okBtn.disabled = false;
      }
    }
    const closed = () => done(null);
    d.addEventListener('submit', submit);
    d.addEventListener('close', closed);
    $('namecancel').onclick = () => done(null);
    input.oninput = () => { err.hidden = true; };
    d.oncancel = (e) => { if (busy) e.preventDefault(); };
    d.showModal();
    input.focus();
    input.select();
  });
}

/** Asks to confirm. Resolves to true when confirmed. */
export function confirmBox({ title, text, ok, danger = false }) {
  const d = $('confirmdlg'), okBtn = $('confirmok');
  $('confirmtitle').textContent = title;
  $('confirmtext').textContent = text;
  okBtn.textContent = ok || t('dlg.ok');
  okBtn.classList.toggle('danger', !!danger);
  okBtn.classList.toggle('primary', !danger);
  return new Promise((resolve) => {
    const done = (v) => {
      d.removeEventListener('close', closed);
      okBtn.onclick = null;
      $('confirmcancel').onclick = null;
      if (d.open) d.close();
      resolve(v);
    };
    const closed = () => done(false);
    d.addEventListener('close', closed);
    okBtn.onclick = () => done(true);
    $('confirmcancel').onclick = () => done(false);
    d.showModal();
    $('confirmcancel').focus();
  });
}
