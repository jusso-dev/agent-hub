const test = require('node:test');
const assert = require('node:assert/strict');
const { boot, tick } = require('./app-harness');

const me = { id: 'ag-me', handle: 'me', display_name: 'Me' };
const bee = { id: 'ag-b', handle: 'bee', display_name: 'Bee' };
const T0 = Date.parse('2026-10-01T00:00:00Z');
const at = (min) => new Date(T0 + min * 60000).toISOString();
let seq = 0;
const post = (where, body, min) => ({ id: `m${++seq}`, from_agent: bee.id, from_handle: 'bee', ...(where.startsWith('#') ? { channel: where.slice(1) } : { to_agent: me.id, to_handle: 'me' }), body, kind: 'chat', created_at: at(min) });

function makeHub() {
  const db = {
    channels: ['general'],
    general: Array.from({ length: 3 }, (_, i) => post('#general', `General ${i}`, i)),
    dms: Array.from({ length: 70 }, (_, i) => post('@me', `DM ${i}`, i)),
    inbox: [],
  };
  const page = (rows, req) => {
    const before = req.query.get('before'); const limit = Number(req.query.get('limit') || 50);
    return rows.filter((m) => !before || m.created_at < before).sort((a, b) => b.created_at.localeCompare(a.created_at)).slice(0, limit);
  };
  const routes = (method, url, req) => {
    const p = req.path;
    if (p === '/api/me') return [200, { agent: me }];
    if (p === '/api/agents') return [200, { agents: [me, bee] }];
    if (p === '/api/channels') return [200, { channels: db.channels.map((name) => ({ name })) }];
    if (p === '/api/inbox') return [200, { items: db.inbox.map((m) => ({ ...m, read_at: null })), unread: db.inbox.length }];
    if (p === '/api/channels/general/messages') return [200, { messages: page(db.general, req) }];
    if (p === '/api/dms/bee/messages') return [200, { messages: page(db.dms, req) }];
    return undefined;
  };
  return { db, routes };
}

test('after the event stream reconnects, inbox, channels and the open conversation are refetched', async (t) => {
  const hub = makeHub();
  const app = await boot({ routes: hub.routes });
  t.after(() => app.close());
  app.w.Math.random = () => 0.5; // backoff of exactly 1 s
  const count = (p) => app.log.filter((r) => r.path === p).length;
  const texts = () => app.$$('.msg-body').map((e) => e.textContent);

  // Open the DM and page back once, then go back to #general.
  app.$('[data-convo="@bee"]').click(); await app.settle(10);
  app.$('.load-older button').click(); await app.settle(10);
  assert.ok(texts().includes('DM 0'), 'all 70 DMs loaded');
  app.$('[data-convo="#general"]').click(); await app.settle(10);
  assert.equal(app.$('#live').dataset.state, 'live');
  const before = { inbox: count('/api/inbox'), channels: count('/api/channels'), general: count('/api/channels/general/messages'), dm: count('/api/dms/bee/messages') };

  // The stream drops, and things happen while it is down.
  hub.db.general.push(post('#general', 'Posted while you were offline', 100));
  const missedDm = post('@me', 'DM sent while you were offline', 101);
  hub.db.dms.push(missedDm); hub.db.inbox.push(missedDm);
  hub.db.channels.push('builds');
  app.streams[0].end();
  await app.settle(5);
  assert.equal(app.$('#live').dataset.state, 'offline');
  assert.ok(!texts().includes('Posted while you were offline'));

  await tick(1300); await app.settle(10);
  assert.equal(app.streams.length, 2, 'reconnected');
  assert.equal(app.$('#live').dataset.state, 'live');
  assert.equal(count('/api/inbox'), before.inbox + 1, 'inbox refetched');
  assert.equal(count('/api/channels'), before.channels + 1, 'channels refetched');
  assert.equal(count('/api/channels/general/messages'), before.general + 1, 'open conversation refetched');
  assert.equal(count('/api/dms/bee/messages'), before.dm, 'closed conversations are not fetched yet');
  assert.ok(texts().includes('Posted while you were offline'), 'missed channel message shown');
  assert.ok(app.$('[data-convo="#builds"]'), 'new channel in the sidebar');
  assert.match(app.$('[data-tab="inbox"] .count')?.textContent || '', /^1$/, 'unread badge counts the missed DM');

  // The DM was marked stale, so opening it refetches the newest page and keeps the older history.
  app.$('[data-convo="@bee"]').click(); await app.settle(10);
  assert.equal(count('/api/dms/bee/messages'), before.dm + 1, 'stale DM refetched when opened');
  assert.ok(texts().includes('DM sent while you were offline'));
  assert.ok(texts().includes('DM 0'), 'older pages still shown');
  assert.equal(app.$('.load-older button'), null, 'still at the start of history, no extra page offered');
  app.$('[data-convo="@bee"]').click(); await app.settle(5);
  assert.equal(count('/api/dms/bee/messages'), before.dm + 1, 'not refetched again once fresh');
});

test('the first connect does not trigger a backfill', async (t) => {
  const hub = makeHub();
  const app = await boot({ routes: hub.routes });
  t.after(() => app.close());
  await app.settle(10);
  assert.equal(app.log.filter((r) => r.path === '/api/channels/general/messages').length, 1);
  assert.equal(app.log.filter((r) => r.path === '/api/channels').length, 1);
});
