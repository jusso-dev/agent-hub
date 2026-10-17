const test = require('node:test');
const assert = require('node:assert/strict');
const { loadMarkdown } = require('./load');

const w = loadMarkdown();
const md = (s) => w.markdown(s);

// Each payload is under the 256 KB body cap. Before the fix, 250k "[" took about
// 70 s in jsdom. Budget is generous so slow CI boxes do not flake.
const BUDGET_MS = 1500;
const N = 250000;
const slow = [
  ['brackets', '['.repeat(N)],
  ['backticks', '`'.repeat(N)],
  ['bracket-label-open', '[a]('.repeat(N / 4)],
  ['backtick-pairs', '``a'.repeat(N / 3)],
  ['stars', '**a'.repeat(N / 3)],
  ['http', 'http://'.repeat(N / 7)],
  ['brackets in short lines', ('['.repeat(3999) + '\n').repeat(60)],
  ['backticks in short lines', ('`'.repeat(3999) + '\n').repeat(60)],
  ['label then long url', ('[a](' + 'b'.repeat(3000) + '\n').repeat(80)],
];
for (const [name, s] of slow) {
  test(`renders ${name} (${s.length} chars) inside ${BUDGET_MS} ms`, () => {
    const t0 = performance.now();
    const el = md(s);
    const ms = performance.now() - t0;
    assert.ok(ms < BUDGET_MS, `${name} took ${Math.round(ms)} ms`);
    assert.equal(el.textContent.replace(/\n/g, ''), s.replace(/\n/g, ''), 'text is preserved');
  });
}

// The plain-text cut-off hides long lines from the regex, so also check the
// regex on its own is linear, with no line limit in front of it.
for (const [name, s] of slow) {
  test(`INLINE regex alone on ${name} inside ${BUDGET_MS} ms`, () => {
    const t0 = performance.now();
    [...s.matchAll(w.INLINE)];
    const ms = performance.now() - t0;
    assert.ok(ms < BUDGET_MS, `${name} took ${Math.round(ms)} ms`);
  });
}

test('lines over 4,000 chars are plain text', () => {
  const long = '**bold** `code` https://example.com ' + 'x'.repeat(4000);
  const el = md(long);
  assert.equal(el.querySelectorAll('strong, code, a').length, 0);
  assert.equal(el.textContent, long);
  const short = md('**bold** `code` https://example.com');
  assert.equal(short.querySelectorAll('strong, code, a').length, 3);
});

test('normal formatting still works', () => {
  const el = md('Use `x` and ``y`` with **care**, see [docs](https://a.com/d) or https://b.com/x.');
  assert.deepEqual([...el.querySelectorAll('code')].map((c) => c.textContent), ['x', 'y']);
  assert.equal(el.querySelector('strong').textContent, 'care');
  assert.deepEqual([...el.querySelectorAll('a')].map((a) => a.getAttribute('href')), ['https://a.com/d', 'https://b.com/x']);
  assert.equal(el.querySelector('a').getAttribute('rel'), 'noopener noreferrer nofollow');
  const fenced = md('```js\nconst a = "<b>";\n```');
  assert.equal(fenced.querySelector('pre code').textContent, 'const a = "<b>";');
  assert.equal(fenced.querySelector('pre code').dataset.lang, 'js');
});

const UNSAFE = 'script,img,svg,iframe,object,embed,style,[onerror],[onload],[onmouseover],[style]';
const unsafeHref = (el) => [...el.querySelectorAll('a')].some((a) => !/^https?:\/\//.test(a.getAttribute('href')));

test('targeted XSS payloads produce no unsafe output', () => {
  const payloads = [
    '[x](javascript:alert(1))', '[x](JAVASCRIPT:alert(1))', '[x](java\tscript:alert(1))', '[x](&#106;avascript:alert(1))',
    '[x](data:text/html,<script>alert(1)</script>)', '[x](vbscript:msgbox(1))', '[x](%6aavascript:alert(1))',
    '[x](\u0000javascript:alert(1))', '[x]( javascript:alert(1))', '<img src=x onerror=alert(1)>', '`<b>x</b>`',
    '**<svg onload=alert(1)>**', 'https://evil.com/"><script>alert(1)</script>', '[click](https://a.com/x"onmouseover="alert(1))',
    '```\n<script>alert(1)</script>\n```', '[x](//evil.com)', '[x](https:evil.com)', '[x](https:\\\\evil.com)', 'HTTPS://x.com',
    '[x](http://a@b.com)', '[x](https://a.com/b(c))',
  ];
  for (const p of payloads) {
    const el = md(p);
    assert.equal(el.querySelector(UNSAFE), null, `unsafe element for ${JSON.stringify(p)}`);
    assert.ok(!unsafeHref(el), `unsafe href for ${JSON.stringify(p)}`);
  }
});

test('random fuzz produces no unsafe output', () => {
  let seed = 0x5e3;
  const rnd = () => { seed = (seed * 1103515245 + 12345) >>> 0; return seed / 2 ** 32; };
  const atoms = ['[', ']', '(', ')', '`', '**', 'javascript:', 'http://', 'https://', 'data:', '<', '>', '"', "'", '&#', '\n', '```', ' ', 'a', '\\', ':', '\t', '%0a'];
  let bad = 0;
  for (let i = 0; i < 20000; i++) {
    let s = ''; const n = 1 + (rnd() * 30 | 0);
    for (let j = 0; j < n; j++) s += atoms[rnd() * atoms.length | 0];
    const el = md(s);
    if (el.querySelector(UNSAFE) || unsafeHref(el)) bad++;
  }
  assert.equal(bad, 0);
});

test('markdownFor caches per id and body', () => {
  w.mdCache.clear();
  const a = w.markdownFor('m1', '**one**');
  const b = w.markdownFor('m1', '**one**');
  assert.notEqual(a, b, 'returns a fresh clone each time');
  assert.equal(b.querySelector('strong').textContent, 'one');
  assert.equal(w.mdCache.size, 1);
  const c = w.markdownFor('m1', '**two**');
  assert.equal(c.querySelector('strong').textContent, 'two', 'an edited body is re-rendered');
  assert.equal(w.mdCache.size, 1);
  const before = w.mdCache.get('m1').node;
  w.markdownFor('m1', '**two**');
  assert.equal(w.mdCache.get('m1').node, before, 'unchanged body is not parsed again');
  assert.equal(w.markdownFor(null, 'x').textContent, 'x');
  assert.equal(w.mdCache.size, 1, 'messages without an id are not cached');
  for (let i = 0; i < 2100; i++) w.markdownFor(`k${i}`, 'x');
  assert.equal(w.mdCache.size, 2000, 'cache is bounded');
});
