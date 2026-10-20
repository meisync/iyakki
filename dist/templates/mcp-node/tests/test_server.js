// Tests with a fake client: it speaks JSON-RPC to the real server process over pipes, as the MCP tile would.
// Run: node tests/test_server.js
'use strict';
const assert = require('assert');
const path = require('path');
const fs = require('fs');
const { spawn } = require('child_process');

const server = path.join(__dirname, '..', 'server.js');

function client() {
  const p = spawn(process.execPath, [server], { stdio: ['pipe', 'pipe', 'inherit'] });
  let buf = '';
  const waiting = [];
  p.stdout.setEncoding('utf8');
  p.stdout.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); const w = waiting.shift(); if (w) w(JSON.parse(l)); }
  });
  let n = 0;
  return {
    call: (method, params) => new Promise((res) => { waiting.push(res); p.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: ++n, method, params: params || {} }) + '\n'); }),
    raw: (text) => new Promise((res) => { waiting.push(res); p.stdin.write(text + '\n'); }),
    close: () => p.kill(),
  };
}

(async () => {
  const c = client();
  try {
    const init = await c.call('initialize', { protocolVersion: '2024-11-05' });
    assert.strictEqual(init.result.protocolVersion, '2024-11-05');
    const names = (await c.call('tools/list')).result.tools.map((t) => t.name).sort();
    assert.deepStrictEqual(names, ['add', 'echo']);
    assert.strictEqual((await c.call('tools/call', { name: 'add', arguments: { a: 2, b: 3 } })).result.content[0].text, '5');
    assert.strictEqual((await c.call('tools/call', { name: 'echo', arguments: { text: 'hi', shout: true } })).result.content[0].text, 'HI');
    for (const args of [{}, { a: 'x', b: 1 }, { a: 1, b: 2, c: 3 }]) {
      assert.ok((await c.call('tools/call', { name: 'add', arguments: args })).error, JSON.stringify(args));
    }
    assert.ok((await c.call('tools/call', { name: 'constructor', arguments: {} })).error, 'inherited names are not tools');
    assert.ok((await c.call('tools/call', { name: 'echo', arguments: { text: 'x'.repeat(5000) } })).error);
    const uri = (await c.call('resources/list')).result.resources[0].uri;
    assert.ok((await c.call('resources/read', { uri })).result.contents[0].text);
    const m = (await c.call('prompts/get', { name: 'summarise', arguments: { text: 'abc' } })).result.messages;
    assert.ok(m[0].content.text.includes('abc'));
    assert.strictEqual((await c.raw('not json')).error.code, -32700);
    assert.deepStrictEqual((await c.call('ping')).result, {});
    assert.strictEqual((await c.call('no/such/method')).error.code, -32601);
    const src = fs.readFileSync(server, 'utf8');
    for (const bad of ['child_process', 'eval(', 'ghp_', 'sk-ant-']) assert.ok(!src.includes(bad), bad);
    console.log('ok');
  } finally {
    c.close();
  }
})().catch((e) => { console.error(e); process.exit(1); });
