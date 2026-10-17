// Boots the whole of web/app.js in jsdom against a scripted fake hub, so the
// tests can drive real UI code paths (paging, live updates, sign-out).
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('jsdom');

const WEB = path.join(__dirname, '..', '..', 'web');
const HTML = fs.readFileSync(path.join(WEB, 'index.html'), 'utf8').replace(/<script[^>]*><\/script>/g, '').replace(/<link rel="stylesheet"[^>]*>/g, '');
const APP = fs.readFileSync(path.join(WEB, 'app.js'), 'utf8');

const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

// One open /api/events response. push() sends raw SSE text, end() closes it.
function sseStream(signal) {
  const enc = new TextEncoder();
  let ctl;
  const body = new ReadableStream({ start(c) { ctl = c; } });
  const s = {
    body, open: true, aborted: false,
    push(text) { if (s.open) ctl.enqueue(enc.encode(text)); },
    end() { if (s.open) { s.open = false; ctl.close(); } },
  };
  signal?.addEventListener('abort', () => { s.aborted = true; if (s.open) { s.open = false; try { ctl.error(new DOMException('aborted', 'AbortError')); } catch { /* closed */ } } });
  return s;
}

function response(status, data) {
  const text = data === undefined ? '' : JSON.stringify(data);
  return { ok: status >= 200 && status < 300, status, statusText: '', body: null, text: async () => text };
}

/**
 * boot({ key, routes, events }) starts the app signed in with `key`.
 * routes(method, url, req) returns [status, json] or undefined (404).
 * events(req) returns a status for /api/events (default 200, which opens a stream).
 */
async function boot({ key = 'ah_test_key', routes, events = () => 200 } = {}) {
  const dom = new JSDOM(HTML, { url: 'https://hub.local/', runScripts: 'outside-only', pretendToBeVisual: true });
  const w = dom.window;
  const log = [];
  const streams = [];
  const pending = new Set();
  w.TextDecoderStream = globalThis.TextDecoderStream;
  w.fetch = (url, opts = {}) => {
    const u = new URL(url, 'https://hub.local/');
    const req = { method: opts.method || 'GET', path: u.pathname, query: u.searchParams, url: u.pathname + u.search, auth: opts.headers?.Authorization, signal: opts.signal, body: opts.body ? JSON.parse(opts.body) : undefined };
    log.push(req);
    if (u.pathname === '/api/events') {
      const st = events(req, streams.length);
      if (st !== 200) return Promise.resolve(response(st, { error: 'nope' }));
      const s = sseStream(opts.signal);
      streams.push(s);
      return Promise.resolve({ ok: true, status: 200, body: s.body });
    }
    const out = routes(req.method, req.url, req) || [404, { error: 'not found' }];
    // Responses can be held back by returning a promise.
    const p = Promise.resolve(out).then(([status, data]) => {
      if (req.signal?.aborted) throw new DOMException('aborted', 'AbortError');
      return response(status, data);
    });
    pending.add(p); p.finally(() => pending.delete(p)).catch(() => {});
    return p;
  };
  w.localStorage.setItem('agenthub.key', key);
  w.eval(APP);
  const settle = async (n = 10) => { for (let i = 0; i < n; i++) { await tick(5); } };
  await settle(20);
  return { w, doc: w.document, log, streams, settle, $: (s) => w.document.querySelector(s), $$: (s) => [...w.document.querySelectorAll(s)], close: () => w.close() };
}

module.exports = { boot, tick };
