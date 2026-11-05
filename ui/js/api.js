// Backend access. Inside Tauri this is the real IPC; in a plain browser (dev preview, screenshots)
// it falls back to the fake backend in ui/dev/mock.js, which is never loaded inside the app.
const tauri = globalThis.__TAURI__;
let mockPromise = null;
const mock = () => (mockPromise ||= import('../dev/mock.js').then((m) => m.createMock()));

export const isMock = !tauri;

export const api = {
  /** Calls a backend command, e.g. invoke('get_library'), invoke('player_seek', { secs }). */
  invoke(cmd, args) {
    return tauri ? tauri.core.invoke(cmd, args) : mock().then((m) => m.invoke(cmd, args));
  },
  /** Subscribes to an event; `cb` receives the payload. Resolves to an unlisten function. */
  listen(event, cb) {
    return tauri ? tauri.event.listen(event, (e) => cb(e.payload)) : mock().then((m) => m.listen(event, cb));
  },
};
