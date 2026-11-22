// Talks to the Rust side (docs/gui-bridge.md). Inside kaltstellen the
// WebView2 bridge `chrome.webview` is used; in a normal browser the mock
// backend from mock.js takes its place, so the page can be designed without
// building Rust.

export const VERSION = 1;

let send = null;
let nextId = 1;
const pending = new Map();
const listeners = new Set();

function dispatch(message) {
  if (!message || message.v !== VERSION) return;
  if (message.event) {
    for (const listener of listeners) listener(message);
    return;
  }
  const resolve = pending.get(message.id);
  if (resolve) {
    pending.delete(message.id);
    resolve(message);
  }
}

/** Connects to the native bridge or, without one, to the mock backend. */
export async function connect() {
  const native = globalThis.chrome && globalThis.chrome.webview;
  if (native) {
    native.addEventListener('message', (event) => dispatch(event.data));
    send = (message) => native.postMessage(message);
    return 'native';
  }
  const mock = await import('./mock.js');
  send = mock.connect(dispatch);
  return 'mock';
}

/**
 * Sends a request and resolves with the reply:
 * `{ ok, state, data?, error? }`.
 */
export function request(type, fields = {}) {
  const id = nextId++;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    send({ v: VERSION, id, type, ...fields });
  });
}

/** One-way notices to the host (`ready`, `drive_done`). */
export function notify(type, fields = {}) {
  send({ v: VERSION, type, ...fields });
}

/** Listens to events pushed by the host (`applied`). */
export function onEvent(listener) {
  listeners.add(listener);
}
