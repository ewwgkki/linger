// Fake backend for browser previews and screenshots (never loaded inside Tauri).
// URL params: ?mock=<song count> (default 400, 0 = empty library), &ffmpeg=0, &lang=en, &theme=dark,
// &pl=0 (start without playlists or favorites), &shot=sel|menu|picker|menu1|queue|qmenu|drag|import|report|ly-word|ly-line|ly-plain|ly-none|ly-retry|np-<same kinds> (screenshots only:
// selects rows 0-3, 5, 6 with synthetic clicks, then opens the context menu, then the "save to" popover from it;
// menu1: plays the first playable row and opens the menu for row 4 alone; queue: plays the list, queues three
// songs and opens the queue panel; qmenu: also right-clicks a queued row; drag: starts dragging row 1 by its
// grip down to row 4 on a playlist page; import: opens settings at the import section; report: runs the mock import
// through the settings button flow and shows the report dialog; ly-<kind>: plays a song with that kind of lyrics
// and opens the lyrics panel (np-<kind>: the full-screen page, also &npcolor=r,g,b|none, &npnocover=1, &npslow=<ms>), with
// &pos=<secs>, &paused=1, &lylong=1 for a 1000-line song).
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

// ---- lyrics (Task 5 commands): generic invented text only ----
const LY_TEXT = [
  ['Morning light on the water', '清晨的光落在水面'], ['We walk where the road runs thin', '我们走在窄窄的路上'],
  ['Counting the lamps as they fade', '数着渐渐熄灭的灯'], ['Carry the quiet with you', '带上这片安静'],
  ['Every window holds a song', '每扇窗里都有一首歌'], ['Slow rivers bring us home', '缓缓的河流带我们回家'],
  ['Hold the last note a while', '把最后一个音留一会儿'], ['Tomorrow answers back', '明天会有回答'],
];
function lyWord(offset, trans) {
  const lines = LY_TEXT.map(([en, zhText], i) => {
    const start = 8000 + i * 6000, parts = en.split(' ');
    const dur = 5200 / parts.length;
    const words = parts.map((w, k) => ({ start: Math.round(start + k * dur), end: Math.round(start + (k + 1) * dur), text: k < parts.length - 1 ? w + ' ' : w }));
    return { start, end: start + 5200, text: en, words, trans: trans ? zhText : null };
  });
  if (new URLSearchParams(location.search).has('lylong')) {
    const long = Array.from({ length: 1000 }, (_, n) => {
      const l = lines[n % lines.length], shift = (n - (n % lines.length)) * 6000;
      return { ...l, start: l.start + shift, end: l.end + shift, words: l.words.map((w) => ({ ...w, start: w.start + shift, end: w.end + shift })) };
    });
    return { status: 'ok', kind: 'word', source: 'netease', lines: long, offset };
  }
  return { status: 'ok', kind: 'word', source: 'netease', lines, offset };
}
function lyLine(offset) {
  const lines = LY_TEXT.map(([en], i) => ({ start: 6000 + i * 7000, end: null, text: en, words: [], trans: null }));
  return { status: 'ok', kind: 'line', source: 'file', lines, offset };
}
function lyPlain(offset) {
  return { status: 'ok', kind: 'plain', source: 'embedded', lines: LY_TEXT.map(([en]) => ({ start: null, end: null, text: en, words: [], trans: null })), offset };
}
const LY_KINDS = ['word', 'line', 'plain', 'none', 'retry', 'word'];

function lyricsCommands(lib) {
  const index = new Map(lib.songs.map((x, i) => [x.id, i]));
  const offsets = new Map(), chosen = new Map(), retried = new Set();
  let settings = { netease: true, trans: true }, slowDone = false;
  const kindOf = (id) => LY_KINDS[(index.get(id) ?? 3) % LY_KINDS.length];
  const get = (id) => {
    const off = offsets.get(id) || 0;
    if (chosen.has(id)) return { id, ...chosen.get(id), offset: off };
    const k = kindOf(id);
    if (k === 'word') return { id, ...lyWord(off, settings.netease) };
    if (k === 'line') return { id, ...lyLine(off) };
    if (k === 'plain') return { id, ...lyPlain(off) };
    if (k === 'retry' && !retried.has(id)) { retried.add(id); return { id, status: 'retry', kind: 'plain', source: '', lines: [], offset: off }; }
    if (k === 'retry') return { id, ...lyLine(off) };
    return { id, status: 'none', kind: 'plain', source: '', lines: [], offset: off };
  };
  return {
    lyrics_get: ({ id }) => get(id),
    lyrics_search: ({ id, title, artist }) => {
      if (title === 'offline') throw 'lyrics.netError';
      const s = lib.songs[index.get(id) ?? 0] || {};
      const t = title ?? (s.t || 'Untitled'), a = artist ?? (s.a || 'Unknown'); // blank stays blank, as in the backend
      return [
        { source: 'netease', ext_id: '1001', title: t, artist: a, album: s.b || '', dur: s.s || 200, synced: true },
        { source: 'netease', ext_id: '1002', title: t + ' (Live)', artist: a, album: 'Live', dur: (s.s || 200) + 12, synced: true },
        { source: 'lrclib', ext_id: '2001', title: t, artist: a, album: s.b || '', dur: s.s || 200, synced: false },
      ];
    },
    lyrics_choose: ({ id, source, extId }) => {
      chosen.set(id, extId === '2001' ? lyPlain(0) : lyWord(0, true));
      chosen.get(id).source = source;
      return get(id);
    },
    lyrics_use_file: ({ id, path }) => {
      if (!/\.(lrc|txt|ttml|qrc|yrc)$/i.test(String(path))) throw 'lyrics.badFile';
      chosen.set(id, lyLine(0));
      return get(id);
    },
    lyrics_set_offset: ({ id, ms }) => { const v = Math.max(-30000, Math.min(30000, Math.round(ms))); offsets.set(id, v); return v; },
    lyrics_settings: () => ({ ...settings }),
    lyrics_set_settings: ({ netease, trans }) => { settings = { netease: !!netease, trans: !!trans }; },
    cover_color: async ({ id }) => {
      const qs = new URLSearchParams(location.search);
      // &npslow=<ms>: the first answer arrives late (a slow reply for the previous song)
      if (qs.has('npslow') && !slowDone) { slowDone = true; await new Promise((r) => setTimeout(r, +qs.get('npslow'))); return [255, 0, 0]; }
      // &npcolor=r,g,b or &npcolor=none: force the cover colour
      if (qs.has('npcolor')) return qs.get('npcolor') === 'none' ? null : qs.get('npcolor').split(',').map(Number);
      const i = index.get(id);
      if (i == null || i % 5 === 4) return null;
      const h = hash(id);
      return [60 + (h & 127), 60 + ((h >> 7) & 127), 60 + ((h >> 14) & 127)];
    },
  };
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
          c, f, br, z: Math.round(s * br * 125), pc, r: root, p, st: 0, av: root !== 3, ad: 1760000000 + songs.length * 3600 });
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
  // the play source (`queue`, position `qi`) plus the user queue `up`, played first (simplified: no
  // shuffle order, the source is kept as given)
  let queue = [], qi = 0, up = [], timer = 0;
  const pushState = () => emit('player://state', { ...ps });
  const emitQueue = () => emit('player://queue', null);
  const playId = (id) => {
    const s = byId.get(id);
    ps.id = s ? s.id : null; ps.pos = 0; ps.dur = s ? s.s : 0; ps.playing = !!s;
    if (s && !ffmpeg && (s.c === 'wma' || s.c === 'ape')) { ps.playing = false; emit('player://error', { id: s.id, reason: 'player.needFfmpeg' }); }
    pushState();
    emitQueue();
  };
  const load = (i) => { qi = i; playId(queue[qi]); };
  window.__mockStop = () => playId(null); // browser checks: playback ends with no current song
  const advance = () => {
    if (up.length) playId(up.shift());
    else if (qi + 1 < queue.length) load(qi + 1);
    else if (ps.repeat === 'all' && queue.length) load(0);
    else { ps.playing = false; pushState(); }
  };
  const part = (p) => { if (p !== 'up' && p !== 'ctx') throw 'queue.badPart'; return p; };
  const ctxLen = () => Math.max(0, queue.length - qi - 1);
  const checkIndex = (p, i) => { if (!(i >= 0 && i < (p === 'up' ? up.length : ctxLen()))) throw 'queue.badIndex'; };
  const checkId = (p, i, id) => { checkIndex(p, i); if ((p === 'up' ? up[i] : queue[qi + 1 + i]) !== id) throw 'queue.badIndex'; };
  clearInterval(timer);
  timer = setInterval(() => {
    if (!ps.playing) return;
    ps.pos = Math.min(ps.dur, ps.pos + 0.25);
    emit('player://pos', { pos: ps.pos });
    if (ps.pos >= ps.dur) advance();
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
    // fixed report: one list with missing songs, one with no match (not created), one unreadable file
    import_spotify: ({ paths }) => {
      if (!paths || !paths.length) throw 'import.empty';
      const ids = lib.songs.slice(0, 14).map((x) => x.id);
      const l = { id: nextList++, name: 'Imported mix', items: ids.map((id, k) => item(id, now() + k)) };
      lists.push(l);
      changed(l.id, false);
      return {
        playlists: [
          { source: 'Imported_mix.csv', name: 'Imported mix', id: l.id, total: 17, matched: 14,
            missing: [{ t: '晴天', a: '周杰伦', b: '叶惠美' }, { t: 'Yesterday', a: 'The Beatles', b: '' }, { t: 'Blue', a: 'Unknown Artist', b: 'Demo' }] },
          { source: 'Nothing_found.csv', name: 'Nothing found', id: null, total: 3, matched: 0,
            missing: [{ t: 'A', a: 'B', b: '' }, { t: 'C', a: 'D', b: 'E' }, { t: 'F', a: 'G', b: '' }] },
        ],
        failed: [{ source: 'D:\\Downloads\\notes.csv', reason: 'import.missingColumns' }],
      };
    },
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
    set_app_title: () => {},
    take_installer_language: () => null,
    export_playlist: ({ id }) => {
      const items = id == null ? fav : findList(id).items;
      globalThis.__lastExport = id;
      return { file: 'demo.m3u8', exported: Math.max(0, items.length - 1), skipped: items.length ? 1 : 0 };
    },
    window_settings: () => ({ close_to_tray: globalThis.__closeToTray !== false }),
    window_set_close_to_tray: ({ enabled }) => { globalThis.__closeToTray = !!enabled; },
    start_scan: ({ root = null } = {}) => {
      globalThis.__lastScanRoot = root;
      emit('library://roots', lib.roots.map((r) => ({ ...r })));
      const total = root == null ? lib.songs.length : lib.songs.filter((s) => s.r === root).length;
      let done = 0;
      const step = setInterval(() => {
        done = Math.min(total, done + Math.max(1, Math.ceil(total / 20)));
        emit('scan://progress', { done, total });
        if (done >= total) {
          clearInterval(step);
          emit('scan://finished', { count: lib.songs.length, root, skipped: [{ path: 'D:\\Music\\notes\\readme.mp3', why: 'skip.notaudio' }, { path: 'D:\\Music\\broken.flac', why: 'skip.unreadable' }] });
        }
      }, 100);
    },
    player_play_list: ({ ids, start, shuffle }) => { queue = ids.slice(); up = []; ps.shuffle = !!shuffle || ps.shuffle; load(start || 0); },
    player_toggle: () => { if (ps.id) { ps.playing = !ps.playing; pushState(); } },
    player_next: () => { if (up.length || queue.length) { const r = ps.repeat; ps.repeat = 'all'; advance(); ps.repeat = r; } },
    player_prev: () => { if (ps.pos > 4) { ps.pos = 0; pushState(); } else if (qi > 0) load(qi - 1); },
    player_seek: ({ secs }) => { ps.pos = Math.max(0, Math.min(ps.dur, secs)); pushState(); },
    player_set_volume: ({ volume }) => { ps.volume = volume; pushState(); },
    player_set_shuffle: ({ on }) => { ps.shuffle = !!on; pushState(); emitQueue(); },
    player_set_repeat: ({ mode }) => { ps.repeat = mode; pushState(); },
    player_queue: () => ({ now: ps.id, up: up.slice(), ctx: queue.slice(qi + 1, qi + 201), ctx_total: ctxLen() }),
    player_queue_add: ({ ids, next }) => {
      if (!ids.length) return;
      if (!ps.id) { queue = ids.slice(); up = []; load(0); return; }
      if (next) up.unshift(...ids);
      else up.push(...ids);
      emitQueue();
    },
    player_queue_remove: ({ part: p, index, id }) => {
      checkId(part(p), index, id);
      if (p === 'up') up.splice(index, 1); else queue.splice(qi + 1 + index, 1);
      emitQueue();
    },
    player_queue_move: ({ part: p, from, to, id }) => {
      checkId(part(p), from, id); checkIndex(p, to);
      const l = p === 'up' ? up : queue, base = p === 'up' ? 0 : qi + 1;
      l.splice(base + to, 0, ...l.splice(base + from, 1));
      emitQueue();
    },
    player_queue_jump: ({ part: p, index, id }) => {
      checkId(part(p), index, id);
      if (p === 'up') { up.splice(0, index); playId(up.shift()); } else load(qi + 1 + index);
    },
    player_queue_clear: () => { up = []; emitQueue(); },
    player_state: () => ({ ...ps }),
    ...lyricsCommands(lib),
  };

  if (qs.get('shot')) setTimeout(() => shot(qs.get('shot'), commands), 1200);

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
function shot(what, commands) {
  const rows = () => [...document.querySelectorAll('#songlist .song')];
  const click = (el, o) => el.dispatchEvent(new MouseEvent('click', { bubbles: true, ...o }));
  if (what.startsWith('ly-') || what.startsWith('np-')) {
    const qs = new URLSearchParams(location.search), kind = what.slice(3);
    const songs = commands.get_library().songs;
    const i = songs.findIndex((s, k) => s.av && LY_KINDS[k % LY_KINDS.length] === kind && (!qs.has('npnocover') || !s.pc));
    if (i < 0) return;
    commands.player_play_list({ ids: [songs[i].id, songs[(i + 1) % songs.length].id], start: 0 });
    if (qs.has('pos')) commands.player_seek({ secs: +qs.get('pos') });
    if (qs.get('paused') === '1') commands.player_toggle();
    setTimeout(() => document.getElementById(what.startsWith('np-') ? 'plcover' : 'lyricsbtn').click(), 150);
    return;
  }
  if (what === 'import' || what === 'report') {
    window.addEventListener('resize', (e) => e.stopImmediatePropagation(), true);
    const link = document.querySelector('#plnav [data-import]');
    (link || document.querySelector('#opensettings')).click();
    if (what === 'report') setTimeout(async () => { const { showReport } = await import('../js/settings.js'); showReport(await commands.import_spotify({ paths: ['x'] })); }, 200);
    return;
  }
  if (rows().length < 8) return;
  // the headless capture resizes the window, which would close the menu / popover
  if (what !== 'sel') window.addEventListener('resize', (e) => e.stopImmediatePropagation(), true);
  if (what === 'queue' || what === 'qmenu') {
    document.querySelector('#playall').click();
    setTimeout(() => {
      const ids = rows().slice(5, 8).map((r) => r.dataset.id);
      commands.player_queue_add({ ids, next: false });
      document.getElementById('queuebtn').click();
      if (what === 'qmenu') {
        // the capture also scrolls the panel, which would close the menu
        document.addEventListener('scroll', (e) => e.stopImmediatePropagation(), true);
        setTimeout(() => {
          const row = document.querySelector('#qup .q-row'), box = row.getBoundingClientRect();
          row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.left + 120, clientY: box.top + 30 }));
        }, 300);
      }
    }, 200);
    return;
  }
  if (what === 'drag') {
    const grip = rows()[1].querySelector('[data-grip]'), box = grip.getBoundingClientRect();
    const opt = (y) => ({ bubbles: true, cancelable: true, button: 0, clientX: box.left + 8, clientY: y, pointerId: 1, pointerType: 'mouse' });
    grip.dispatchEvent(new PointerEvent('pointerdown', opt(box.top + 10)));
    const rowH = rows()[1].offsetHeight;
    document.dispatchEvent(new PointerEvent('pointermove', opt(box.top + 10 + rowH * 2.7)));
    return;
  }
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
