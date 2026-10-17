const test = require('node:test');
const assert = require('node:assert/strict');
const { boot } = require('./app-harness');

const me = { id: 'ag-me', handle: 'me', display_name: 'Me' };
const bee = { id: 'ag-b', handle: 'bee', display_name: 'Bee' };
const T0 = Date.parse('2026-10-01T00:00:00Z');
const at = (min) => new Date(T0 + min * 60000).toISOString();
const dm = (n, min) => ({ id: `dm-${n}`, from_agent: bee.id, from_handle: 'bee', to_agent: me.id, to_handle: 'me', body: `DM ${n}`, kind: 'chat', created_at: at(min) });

// 300 DMs from @bee, one a minute, starting three weeks after an old one.
// The old DM (#0) is still in the inbox, so it sits in the local map from the start.
const OLD = dm(0, -3 * 7 * 24 * 60);
const DMS = [OLD, ...Array.from({ length: 300 }, (_, i) => dm(i + 1, i))];

function hub() {
  return (method, url, req) => {
    const p = req.path;
    if (p === '/api/me') return [200, { agent: me }];
    if (p === '/api/agents') return [200, { agents: [me, bee] }];
    if (p === '/api/channels') return [200, { channels: [] }];
    if (p === '/api/inbox') return [200, { items: [{ ...OLD, read_at: null }], unread: 1 }];
    if (p === '/api/dms/bee/messages') {
      const before = req.query.get('before'); const limit = Number(req.query.get('limit'));
      const page = DMS.filter((m) => !before || m.created_at < before).sort((a, b) => b.created_at.localeCompare(a.created_at)).slice(0, limit);
      return [200, { with: 'bee', messages: page }];
    }
    return undefined;
  };
}

test('load earlier messages pages from the server cursor, not the oldest local message', async (t) => {
  const app = await boot({ routes: hub() });
  t.after(() => app.close());
  app.$('[data-convo="@bee"]')?.click();
  await app.settle(20);
  const bodies = () => app.$$('.msg-body').map((e) => e.textContent);
  assert.ok(bodies().includes('DM 300'), 'newest page shown');
  assert.ok(bodies().includes('DM 0'), 'old inbox DM is held locally');
  const befores = () => app.log.filter((r) => r.path === '/api/dms/bee/messages').map((r) => r.query.get('before'));
  for (let i = 0; i < 6 && app.$('.load-older button'); i++) { app.$('.load-older button').click(); await app.settle(10); }
  const seen = new Set(bodies());
  const missing = DMS.filter((m) => !seen.has(m.body)).map((m) => m.body);
  assert.deepEqual(missing, [], 'every DM is reachable');
  const b = befores();
  assert.equal(b[0], null, 'first page has no cursor');
  assert.equal(b[1], at(250), 'second page starts before the oldest message of the first page');
  assert.ok(!b.includes(OLD.created_at), 'never pages from the old inbox message');
  assert.equal(app.$('.load-older button'), null, 'button goes once history is exhausted');
});
