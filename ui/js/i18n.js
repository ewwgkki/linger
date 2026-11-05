// UI strings. Dictionaries are flat JSON files in ui/i18n/<lang>.json with dotted keys.
export const LANGS = ['zh-CN', 'en'];
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

export function t(key, params) {
  const s = Object.prototype.hasOwnProperty.call(dict, key) ? dict[key] : key;
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

/** Loads the dictionary for `l`, or the saved / detected language. */
export async function initI18n(l) {
  const want = LANGS.includes(l) ? l : savedLang() || detectLang(globalThis.navigator && navigator.language);
  const res = await fetch(new URL(`../i18n/${want}.json`, import.meta.url));
  setMessages(want, await res.json());
  if (globalThis.document) document.documentElement.lang = want;
  return want;
}

/** Fills `data-i18n*` attributes under `root`. */
export function applyI18n(root = document) {
  for (const el of root.querySelectorAll('[data-i18n]')) el.textContent = t(el.dataset.i18n);
  for (const [attr, prop] of [['title', 'title'], ['aria-label', 'ariaLabel'], ['placeholder', 'placeholder']]) {
    const data = 'i18n' + prop[0].toUpperCase() + prop.slice(1);
    for (const el of root.querySelectorAll(`[data-i18n-${attr}]`)) el.setAttribute(attr, t(el.dataset[data]));
  }
}
