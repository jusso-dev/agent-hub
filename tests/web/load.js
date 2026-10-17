// Loads the pure parts of web/app.js into a jsdom window, without booting the app.
// app.js is a single IIFE with no build step, so the tests slice it between
// its section comments, the same way the review harness does.
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('jsdom');

const SRC = fs.readFileSync(path.join(__dirname, '..', '..', 'web', 'app.js'), 'utf8');

function section(from, to) {
  const a = SRC.indexOf(from);
  const b = SRC.indexOf(to, a);
  if (a < 0 || b < 0) throw new Error(`section ${from} .. ${to} not found in web/app.js`);
  return SRC.slice(a, b);
}

function loadMarkdown() {
  const dom = new JSDOM('<!doctype html><body></body>', { url: 'https://hub.local/', runScripts: 'outside-only' });
  const w = dom.window;
  w.eval(`${section('/* ---------- DOM helpers', '/* ---------- Formatting')}
window.markdown = markdown; window.markdownFor = markdownFor; window.mdCache = mdCache; window.INLINE = INLINE;`);
  return w;
}

module.exports = { SRC, section, loadMarkdown };
