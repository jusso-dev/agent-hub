const test = require('node:test');
const assert = require('node:assert/strict');
const { boot, tick } = require('./app-harness');

const KEY = 'ah_test_key';
const me = { id: 'ag-me', handle: 'me', display_name: 'Me' };
const bee = { id: 'ag-b', handle: 'bee', display_name: 'Bee' };

// A hub where `revoke()` makes every call with the key return 401, as #4 does.
function makeHub() {
  const hub = { revoked: false, hold: null };
  hub.routes = (method, url, req) => {
    if (hub.revoked) return [401, { error: 'invalid or revoked key' }];
    const p = req.path;
    if (p === '/api/me') return [200, { agent: me }];
    if (p === '/api/agents') return [200, { agents: [me, bee] }];
    if (p === '/api/channels') return [200, { channels: [{ name: 'general' }] }];
    if (p === '/api/inbox') return [200, { items: [], unread: 0 }];
    if (p === '/api/channels/general/messages') return [200, { messages: [] }];
    if (p === '/api/dms/bee/messages') return hub.hold || [200, { messages: [] }];
    if (p === '/api/messages' && method === 'POST') return [403, { error: 'not allowed' }];
    return undefined;
  };
  hub.events = () => (hub.revoked ? 401 : 200);
  return hub;
}

async function start(t) {
  const hub = makeHub();
  const app = await boot({ key: KEY, routes: hub.routes, events: hub.events });
  t.after(() => app.close());
  app.w.Math.random = () => 0.5;
  assert.equal(app.$('#app').hidden, false, 'signed in');
  return { hub, app };
}
function assertSignedOut(app) {
  assert.equal(app.$('#app').hidden, true, 'app hidden');
  assert.equal(app.$('#login').hidden, false, 'login shown');
  assert.equal(app.w.localStorage.getItem('agenthub.key'), null, 'saved key cleared');
  assert.equal(app.$('#login-error').hidden, false);
  assert.match(app.$('#login-error').textContent, /may have been revoked/);
}
const eventsCalls = (app) => app.log.filter((r) => r.path === '/api/events').length;

test('a revoked event for this agent signs out, aborts requests and stops reconnecting', async (t) => {
  const { hub, app } = await start(t);
  // Leave a request in flight that only answers after sign-out.
  let answer; hub.hold = new Promise((r) => { answer = r; });
  app.$('[data-convo="@bee"]').click(); await app.settle(3);
  const held = app.log.find((r) => r.path === '/api/dms/bee/messages');
  app.streams[0].push(`event: revoked\ndata: {"id":"${me.id}"}\n\n`);
  app.streams[0].end();
  await app.settle(5);
  assertSignedOut(app); // from the event alone: no call has had a 401 yet
  hub.revoked = true;
  assert.equal(held.aborted, true, 'in-flight request aborted');
  assert.equal(app.streams[0].aborted || !app.streams[0].open, true, 'stream closed');
  answer([200, { messages: [{ id: 'late', from_agent: bee.id, from_handle: 'bee', to_agent: me.id, body: 'late reply', created_at: new Date().toISOString() }] }]);
  const n = eventsCalls(app);
  await tick(2500); await app.settle(5);
  assert.equal(eventsCalls(app), n, 'no reconnect attempts after sign-out');
  assert.equal(app.$$('.toast').length, 0, 'no error toasts on the login page');
  assert.equal(app.doc.body.textContent.includes('late reply'), false, 'late response dropped');
});

test('a 401 from the event stream reconnect signs out and stops retrying', async (t) => {
  const { hub, app } = await start(t);
  hub.revoked = true;
  app.streams[0].end(); // stream dropped without a revoked event
  await tick(1300); await app.settle(5);
  assertSignedOut(app);
  const n = eventsCalls(app);
  assert.equal(n, 2, 'one reconnect, which got the 401');
  await tick(2500); await app.settle(5);
  assert.equal(eventsCalls(app), n, 'no more reconnects');
});

test('a 401 from any API call signs out', async (t) => {
  const { hub, app } = await start(t);
  hub.revoked = true;
  app.$('[data-tab="inbox"]').click();
  await app.settle(10);
  assertSignedOut(app);
  assert.equal(app.streams[0].aborted, true, 'live stream aborted');
  assert.equal(app.$$('.toast').length, 0, 'no toast, the login error explains it');
});

test('a revoked event for another agent, and a 403, do not sign out', async (t) => {
  const { app } = await start(t);
  app.streams[0].push('event: revoked\ndata: {"id":"ag-someone-else"}\n\n');
  await app.settle(5);
  assert.equal(app.$('#app').hidden, false, 'still signed in');
  app.$('#composer-input').value = 'hello';
  app.$('#composer-input').dispatchEvent(new app.w.Event('input'));
  app.$('form.composer').requestSubmit();
  await app.settle(10);
  assert.equal(app.$('#app').hidden, false, 'still signed in after a 403');
  assert.match(app.$('.toast')?.textContent || '', /not allowed to post/);
  assert.equal(app.w.localStorage.getItem('agenthub.key'), KEY, 'key kept');
});

test('a revoked saved key on page load still shows the sign-in error', async (t) => {
  const hub = makeHub(); hub.revoked = true;
  const app = await boot({ key: KEY, routes: hub.routes, events: hub.events });
  t.after(() => app.close());
  assertSignedOut(app);
  assert.equal(eventsCalls(app), 0, 'never opened the stream');
});
