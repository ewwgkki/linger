// Fake backend for browser previews and screenshots (never loaded inside Tauri).
// URL params: ?mock=<song count> (default 400, 0 = empty library), &ffmpeg=0, &lang=en, &theme=dark,
// &pl=0 (start without playlists or favorites), &shot=sel|menu|picker|menu1 (screenshots only: selects rows
// 0-3, 5, 6 with synthetic clicks, then opens the context menu, then the "save to" popover from it;
// menu1: plays the first playable row and opens the menu for row 4 alone).
import { setCoverSource } from '../js/util.js';

function rng(seed) {
  let x = seed >>> 0 || 1;
  return () => ((x = (x * 1664525 + 1013904223) >>> 0) / 4294967296);
}

const SYL = ['ka', 'lo', 'mi', 'ra', 'ven', 'sol', 'ta', 'ne', 'or', 'li', 'an', 'el', 'do', 'ri', 'sa', 'mu', 'ber', 'den', 'fa', 'ion'];
const ZH = '风月光年夜海城雨歌心梦花远方星空故事天下河山春秋时间旅人白色南北青';
const word = (r, n) => Array.from({ length: n }, () => SYL[Math.floor(r() * SYL.length)]).join('');
const cap = (s) => s[0].toUpperCase() + s.slice(1);
const zh = (r, n) => Array.from({ length: n }, () => ZH[Math.floor(r() * ZH.length)]).join('');

function hash(s) {
  let h = 2166136261;
  for (const c of s) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  return h >>> 0;
}

function build(count, latin) {
  const r = rng(42);
  const roots = [
    { id: 1, path: 'D:\\Music', online: true, count: 0 },
    { id: 2, path: '\\\\NAS\\music\\Archive', online: true, count: 0 },
    { id: 3, path: 'E:\\Old Rips', online: false, count: 0 },
  ];
  const codecs = [
    ['mp3', 'mp3', 320], ['mp3', 'mp3', 192], ['mp3', 'mp3', 128], ['flac', 'flac', 950], ['flac', 'flac', 1020],
    ['aac', 'm4a', 256], ['pcm', 'wav', 1411], ['wma', 'wma', 192], ['ape', 'ape', 880], ['alac', 'm4a', 780],
  ];
  const songs = [];
  let ai = 0;
  while (songs.length < count) {
    const useZh = !latin && r() < 0.5;
    const artist = r() < 0.03 ? '' : useZh ? zh(r, 2 + Math.floor(r() * 2)) : cap(word(r, 2)) + (r() < 0.4 ? ' ' + cap(word(r, 2)) : '');
    const albums = 1 + Math.floor(r() * 4);
    for (let b = 0; b < albums && songs.length < count; b++) {
      const album = r() < 0.05 ? '' : useZh ? zh(r, 2 + Math.floor(r() * 3)) : cap(word(r, 2 + Math.floor(r() * 2))) + (r() < 0.3 ? ' ' + cap(word(r, 2)) : '');
      const [c, f, br0] = codecs[Math.floor(r() * codecs.length)];
      const root = r() < 0.6 ? 1 : r() < 0.75 ? 2 : 3;
      const pc = r() < 0.75 ? (r() < 0.5 ? 2 : 1) : 0;
      const tracks = 6 + Math.floor(r() * 9);
      for (let n = 1; n <= tracks && songs.length < count; n++) {
        const t = useZh ? zh(r, 2 + Math.floor(r() * 5)) : cap(word(r, 1 + Math.floor(r() * 3))) + (r() < 0.35 ? ' ' + word(r, 2) : '');
        const s = 150 + r() * 270;
        const br = br0 + (c === 'flac' ? Math.floor(r() * 200) : 0);
        const p = `${artist || 'Unknown'}/${album || 'Singles'}/${String(n).padStart(2, '0')} ${t}.${f}`;
        songs.push({ id: (hash(p + ai) >>> 0).toString(16).padStart(8, '0') + songs.length.toString(16), t, a: artist, b: album, n, s,
          c, f, br, z: Math.round(s * br * 125), pc, r: root, p, st: 0, av: root !== 3 });
      }
    }
    ai++;
  }
  for (const rt of roots) rt.count = songs.filter((s) => s.r === rt.id).length;
  return { roots: count ? roots : [], songs };
}

function coverSvg(seed) {
  const h1 = seed % 360, h2 = (h1 + 40 + (seed >> 9) % 80) % 360;
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><defs><linearGradient id='g' x1='0' y1='0' x2='1' y2='1'>` +
    `<stop offset='0' stop-color='hsl(${h1},55%,58%)'/><stop offset='1' stop-color='hsl(${h2},50%,32%)'/></linearGradient></defs>` +
    `<rect width='100' height='100' fill='url(#g)'/><circle cx='${30 + seed % 40}' cy='${30 + (seed >> 4) % 40}' r='${12 + seed % 18}' fill='rgba(255,255,255,.18)'/></svg>`;
  return 'data:image/svg+xml,' + encodeURIComponent(svg);
}

export function createMock() {
  const qs = new URLSearchParams(location.search);
  const count = qs.has('mock') ? Math.max(0, parseInt(qs.get('mock'), 10) || 0) : 400;
  const ffmpeg = qs.get('ffmpeg') !== '0';
  const lib = build(count, qs.get('lang') === 'en');
  const byId = new Map(lib.songs.map((s) => [s.id, s]));
  setCoverSource((s) => coverSvg(hash(s.a + '\u0001' + s.b)));

  const subs = new Map();
  const emit = (ev, payload) => (subs.get(ev) || []).forEach((cb) => setTimeout(() => cb(payload)));
  const ps = { id: null, playing: false, pos: 0, dur: 0, shuffle: false, repeat: 'off', volume: 100 };
  let queue = [], qi = 0, timer = 0;
  const pushState = () => emit('player://state', { ...ps });
  const load = (i) => {
    qi = i;
    const s = byId.get(queue[qi]);
    ps.id = s ? s.id : null; ps.pos = 0; ps.dur = s ? s.s : 0; ps.playing = !!s;
    if (s && !ffmpeg && (s.c === 'wma' || s.c === 'ape')) { ps.playing = false; emit('player://error', { id: s.id, reason: 'player.needFfmpeg' }); }
    pushState();
  };
  clearInterval(timer);
  timer = setInterval(() => {
    if (!ps.playing) return;
    ps.pos = Math.min(ps.dur, ps.pos + 0.25);
    emit('player://pos', { pos: ps.pos });
    if (ps.pos >= ps.dur) { if (qi + 1 < queue.length) load(qi + 1); else { ps.playing = false; pushState(); } }
  }, 250);

  // ---------- playlists and favorites (in memory; favorites are target 0 in save_songs) ----------
  const now = () => Math.floor(Date.now() / 1000);
  const snap = (id) => { const x = byId.get(id); return x ? { t: x.t, a: x.a, b: x.b, s: x.s } : null; };
  let nextItem = 1, nextList = 1;
  const lists = [], fav = [];
  const item = (id, ad, extra) => ({ i: nextItem++, id, ad, ...(snap(id) || extra) });
  const findList = (id) => { const l = lists.find((x) => x.id === id); if (!l) throw 'playlist.notFound'; return l; };
  const validName = (name) => {
    const n = String(name || '').trim();
    if (!n) throw 'playlist.nameEmpty';
    if ([...n].length > 100) throw 'playlist.nameTooLong';
    return n;
  };
  const dto = (l) => ({ id: l.id, name: l.name, count: l.items.length, dur: l.items.reduce((a, x) => a + x.s, 0) });
  const changed = (id, favorites) => emit('playlists://changed', { id, favorites });
  const addTo = (items, ids) => {
    let added = 0, duplicate = 0;
    for (const id of ids) {
      if (!byId.has(id)) continue;
      if (items.some((x) => x.id === id)) { duplicate++; continue; }
      items.push(item(id, now())); added++;
    }
    return { added, duplicate };
  };
  const removeFrom = (items, ids) => {
    let n = 0;
    for (const id of ids) { const k = items.findIndex((x) => x.id === id); if (k >= 0) { items.splice(k, 1); n++; } }
    return n;
  };
  if (qs.get('pl') !== '0' && lib.songs.length) {
    const t0 = now() - 86400 * 30;
    const pick = (from, n, step) => Array.from({ length: n }, (_, k) => lib.songs[(from + k * step) % lib.songs.length].id);
    const seed = [['Late night', pick(3, 24, 7)], [qs.get('lang') === 'en' ? 'Weekend mornings' : '周末早晨', pick(40, 9, 13)], ['Road trip', pick(11, 40, 5)]];
    for (const [name, ids] of seed) {
      const l = { id: nextList++, name, items: ids.map((id, k) => item(id, t0 + k * 600)) };
      lists.push(l);
    }
    // a song that has left the library: only its snapshot remains
    lists[0].items.splice(2, 0, item('deadbeef0', t0 + 30, { t: 'Gone Song', a: 'Nobody', b: 'Lost Album', s: 201 }));
    for (const [k, id] of pick(5, 14, 11).entries()) fav.unshift(item(id, t0 + k * 3600));
  }

  const commands = {
    get_playlists: () => lists.map(dto),
    get_playlist: ({ id }) => findList(id).items.map((x) => ({ ...x })),
    get_favorites: () => fav.map((x) => ({ ...x })),
    get_saved: () => [...new Set([...fav, ...lists.flatMap((l) => l.items)].map((x) => x.id))],
    get_memberships: ({ songIds }) => {
      const ids = new Set(songIds);
      const n = (items) => items.filter((x) => ids.has(x.id)).length;
      return { total: ids.size, fav: n(fav), lists: lists.map((l) => [l.id, n(l.items)]).filter(([, c]) => c) };
    },
    create_playlist: ({ name, songIds = [] }) => {
      const l = { id: nextList++, name: validName(name), items: [] };
      addTo(l.items, songIds);
      lists.push(l);
      changed(l.id, false);
      return dto(l);
    },
    rename_playlist: ({ id, name }) => { const l = findList(id); l.name = validName(name); changed(id, false); },
    delete_playlist: ({ id }) => { findList(id); lists.splice(lists.findIndex((l) => l.id === id), 1); changed(id, false); },
    playlist_add: ({ id, songIds }) => { const r = addTo(findList(id).items, songIds); changed(id, false); return r; },
    playlist_remove: ({ id, itemIds }) => { const l = findList(id); l.items = l.items.filter((x) => !itemIds.includes(x.i)); changed(id, false); },
    playlist_move: ({ id, itemId, to }) => {
      const l = findList(id), k = l.items.findIndex((x) => x.i === itemId);
      if (k < 0) return;
      const [x] = l.items.splice(k, 1);
      l.items.splice(Math.max(0, Math.min(l.items.length, to)), 0, x);
      changed(id, false);
    },
    set_favorite: ({ songIds, on }) => {
      const n = on ? addTo(fav, songIds).added : removeFrom(fav, songIds);
      fav.sort((a, b) => b.ad - a.ad || b.i - a.i);
      changed(null, true);
      return n;
    },
    save_songs: ({ songIds, add = [], remove = [] }) => {
      const r = { added: 0, duplicate: 0, removed: 0 };
      for (const id of add) {
        const o = addTo(id === 0 ? fav : findList(id).items, songIds);
        r.added += o.added; r.duplicate += o.duplicate;
      }
      for (const id of remove) r.removed += removeFrom(id === 0 ? fav : findList(id).items, songIds);
      fav.sort((a, b) => b.ad - a.ad || b.i - a.i);
      changed(null, true);
      return r;
    },

    get_library: () => ({ roots: lib.roots.map((r) => ({ ...r })), songs: lib.songs, scanned_at: count ? Math.floor(Date.now() / 1000) - 3600 : null, ffmpeg }),
    add_roots: ({ paths }) => {
      for (const p of paths) if (!lib.roots.some((r) => r.path === p)) lib.roots.push({ id: Math.max(0, ...lib.roots.map((r) => r.id)) + 1, path: p, online: true, count: 0 });
      return lib.roots.map((r) => ({ ...r }));
    },
    remove_root: ({ id }) => {
      lib.roots = lib.roots.filter((r) => r.id !== id);
      return lib.roots.map((r) => ({ ...r }));
    },
    start_scan: () => {
      emit('library://roots', lib.roots.map((r) => ({ ...r })));
      const total = lib.songs.length;
      let done = 0;
      const step = setInterval(() => {
        done = Math.min(total, done + Math.max(1, Math.ceil(total / 20)));
        emit('scan://progress', { done, total });
        if (done >= total) {
          clearInterval(step);
          emit('scan://finished', { count: total, skipped: [{ path: 'D:\\Music\\notes\\readme.mp3', why: 'skip.notaudio' }, { path: 'D:\\Music\\broken.flac', why: 'skip.unreadable' }] });
        }
      }, 100);
    },
    player_play_list: ({ ids, start, shuffle }) => { queue = ids.slice(); ps.shuffle = !!shuffle || ps.shuffle; load(start || 0); },
    player_toggle: () => { if (ps.id) { ps.playing = !ps.playing; pushState(); } },
    player_next: () => { if (queue.length) load((qi + 1) % queue.length); },
    player_prev: () => { if (ps.pos > 4) { ps.pos = 0; pushState(); } else if (qi > 0) load(qi - 1); },
    player_seek: ({ secs }) => { ps.pos = Math.max(0, Math.min(ps.dur, secs)); pushState(); },
    player_set_volume: ({ volume }) => { ps.volume = volume; pushState(); },
    player_set_shuffle: ({ on }) => { ps.shuffle = !!on; pushState(); },
    player_set_repeat: ({ mode }) => { ps.repeat = mode; pushState(); },
    player_queue_add: ({ ids, next }) => {
      if (!ps.id) { queue = ids.slice(); load(0); return; }
      if (next) queue.splice(qi + 1, 0, ...ids);
      else queue.push(...ids);
    },
    player_state: () => ({ ...ps }),
  };

  if (qs.get('shot')) setTimeout(() => shot(qs.get('shot')), 1200);

  return {
    async invoke(cmd, args = {}) {
      const f = commands[cmd];
      if (!f) throw new Error(`mock: unknown command ${cmd}`);
      return f(args);
    },
    async listen(ev, cb) {
      if (!subs.has(ev)) subs.set(ev, []);
      subs.get(ev).push(cb);
      return () => subs.set(ev, subs.get(ev).filter((x) => x !== cb));
    },
  };
}

/** Screenshot setup: there is no right click in a headless browser, so drive the UI with synthetic events. */
function shot(what) {
  const rows = () => [...document.querySelectorAll('#songlist .song')];
  const click = (el, o) => el.dispatchEvent(new MouseEvent('click', { bubbles: true, ...o }));
  if (rows().length < 8) return;
  // the headless capture resizes the window, which would close the menu / popover
  if (what !== 'sel') window.addEventListener('resize', (e) => e.stopImmediatePropagation(), true);
  if (what === 'menu1') {
    // one song: play another one first (player bar heart), then right-click a row
    const pb = document.querySelector('#songlist [data-play]');
    if (pb) click(pb);
    const row = rows()[4], box = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.left + 260, clientY: box.top + 30 }));
    return;
  }
  // re-query each time: a click re-renders the rows
  click(rows()[0], { ctrlKey: true });
  click(rows()[3], { shiftKey: true });
  click(rows()[5], { ctrlKey: true });
  click(rows()[6], { ctrlKey: true });
  if (what === 'sel') return;
  const row = rows()[1], box = row.getBoundingClientRect();
  row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.left + 260, clientY: box.top + 30 }));
  if (what === 'picker') setTimeout(() => document.querySelector('#menu [data-act="save"]').click(), 50);
}
