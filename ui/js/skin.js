// Visual themes. Light or dark still follows the system inside each theme.
export const SKINS = ['classic', 'fresh'];
const KEY = 'linger.skin';

export const normalizeSkin = (v) => (SKINS.includes(v) ? v : 'classic');

export function currentSkin(root = document.documentElement) {
  return normalizeSkin(root.dataset.skin);
}

/** Applies and saves a theme. Returns the theme that was applied. */
export function setSkin(value, root = document.documentElement, storage = globalThis.localStorage) {
  const skin = normalizeSkin(value);
  if (skin === 'classic') delete root.dataset.skin;
  else root.dataset.skin = skin;
  try { storage.setItem(KEY, skin); } catch (_) { /* storage unavailable */ }
  return skin;
}
