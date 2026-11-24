// UI strings. Dictionaries are flat JSON files in ui/i18n/<lang>.json with dotted keys.
export const LANGS = ['zh-CN', 'en'];
/** Each language's own name. */
export const LANG_NAMES = { 'zh-CN': '\u7b80\u4f53\u4e2d\u6587', en: 'English' };
const LANG_KEY = 'linger.lang';

let current = 'zh-CN';
let dict = {};

export function detectLang(navLang) {
  return String(navLang || '').toLowerCase().startsWith('zh') ? 'zh-CN' : 'en';
}

export function lang() {
  return current;
}

/** Installs a dictionary directly (tests, or after loading). */
export function setMessages(l, messages) {
  current = l;
  dict = messages || {};
}

const has = (k) => Object.prototype.hasOwnProperty.call(dict, k);

let mac = /^Mac/.test((globalThis.navigator && navigator.platform) || '');
/** Tests can pretend to run on macOS. */
export function setMac(v) { mac = !!v; }

/** On macOS `key.mac` replaces `key`. `params.n === 1` uses `key_one` when the dictionary has it (en singulars). */
export function t(key, params) {
  if (mac && has(key + '.mac')) key += '.mac';
  if (params && params.n === 1 && has(key + '_one')) key += '_one';
  const s = has(key) ? dict[key] : key;
  if (!params) return s;
  return s.replace(/\{(\w+)\}/g, (m, k) => (k in params ? String(params[k]) : m));
}

function savedLang() {
  try {
    const v = localStorage.getItem(LANG_KEY);
    if (LANGS.includes(v)) return v;
  } catch (_) { /* storage unavailable */ }
  return null;
}

/** Makes `l` the saved language, as if it had been picked in Settings. */
export function rememberLanguage(l) {
  if (!LANGS.includes(l)) return;
  try { localStorage.setItem(LANG_KEY, l); } catch (_) { /* storage unavailable */ }
}

const loadDict = async (l) => {
  const res = await fetch(new URL(`../i18n/${l}.json`, import.meta.url));
  if (!res.ok) throw new Error(`language file ${l}: ${res.status}`);
  return res.json();
};

/** Loads the dictionary for `l`, or the saved / detected language. */
export async function initI18n(l) {
  const want = LANGS.includes(l) ? l : savedLang() || detectLang(globalThis.navigator && navigator.language);
  setMessages(want, await loadDict(want));
  if (globalThis.document) document.documentElement.lang = want;
  return want;
}

const listeners = new Set();
/** Calls `fn(lang)` after each language switch. Returns the unsubscribe function. */
export function onLanguage(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

let switchSeq = 0;
/** Switches the interface language now and remembers it. True if the language changed. */
export async function setLanguage(l, load = loadDict) {
  if (!LANGS.includes(l)) return false;
  const seq = ++switchSeq;
  if (l === current) return false;
  const messages = await load(l);
  if (seq !== switchSeq) return false; // a newer choice was made meanwhile
  setMessages(l, messages);
  try { localStorage.setItem(LANG_KEY, l); } catch (_) { /* storage unavailable */ }
  if (globalThis.document) {
    document.documentElement.lang = l;
    applyI18n();
  }
  for (const fn of listeners) {
    try { fn(l); } catch (e) { console.error('language listener', e); }
  }
  return true;
}

/** Fills `data-i18n*` attributes under `root`. */
export function applyI18n(root = document) {
  for (const el of root.querySelectorAll('[data-i18n]')) el.textContent = t(el.dataset.i18n);
  for (const [attr, prop] of [['title', 'title'], ['aria-label', 'ariaLabel'], ['placeholder', 'placeholder']]) {
    const data = 'i18n' + prop[0].toUpperCase() + prop.slice(1);
    for (const el of root.querySelectorAll(`[data-i18n-${attr}]`)) el.setAttribute(attr, t(el.dataset[data]));
  }
}
