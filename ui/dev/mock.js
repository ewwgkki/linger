// Fake backend for browser previews and screenshots (never loaded inside Tauri).
// URL params: ?mock=<song count> (default 400, 0 = empty library), &ffmpeg=0, &lang=en, &theme=dark.
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

  const commands = {
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
    player_state: () => ({ ...ps }),
  };

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
