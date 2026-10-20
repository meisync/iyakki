#!/usr/bin/env node
// __TITLE__: an MCP server over stdio (newline-delimited JSON-RPC 2.0). Node built-ins only, no packages.
// Add your tools to TOOLS. Rules this file follows: validate every argument against the schema, never start a
// shell, bound the time and the size of a tool call, stdout carries protocol messages only (logs go to stderr),
// secrets come from the environment and never from the code.
'use strict';

const PROTOCOL = '2024-11-05';
const NAME = '__NAME__';
const VERSION = '0.1.0';
const MAX_LINE = 1000000;
const MAX_RESULT = 100000;
const TOOL_MS = 10000;

const TOOLS = {
  echo: {
    description: 'Return the text, optionally in capitals.',
    schema: { type: 'object', properties: { text: { type: 'string', maxLength: 2000 }, shout: { type: 'boolean' } }, required: ['text'] },
    run: async (a) => (a.shout ? a.text.toUpperCase() : a.text),
  },
  add: {
    description: 'Add two numbers.',
    schema: { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] },
    run: async (a) => String(a.a + a.b),
  },
};

const RESOURCES = { 'notes://readme': { name: 'About this server', mime: 'text/plain', text: '__TITLE__ is an example MCP server.' } };
const PROMPTS = { summarise: { description: 'Ask for a short summary of a text.', arguments: [{ name: 'text', required: true }], text: 'Summarise this in three bullet points:\n{text}' } };

class BadArgs extends Error {}

const typeOk = (t, v) => ({ string: typeof v === 'string', boolean: typeof v === 'boolean', number: typeof v === 'number' && Number.isFinite(v), integer: Number.isInteger(v), object: v !== null && typeof v === 'object' && !Array.isArray(v), array: Array.isArray(v) })[t];

function checkArgs(schema, args) {
  if (args === null || typeof args !== 'object' || Array.isArray(args)) throw new BadArgs('arguments must be an object');
  const props = schema.properties || {};
  for (const r of schema.required || []) if (!(r in args)) throw new BadArgs(`missing argument ${r}`);
  for (const [k, v] of Object.entries(args)) {
    if (!(k in props)) throw new BadArgs(`unknown argument ${k}`);
    if (props[k].type && !typeOk(props[k].type, v)) throw new BadArgs(`${k} must be ${props[k].type}`);
    if (typeof v === 'string' && v.length > (props[k].maxLength || 10000)) throw new BadArgs(`${k} is too long`);
  }
}

function bounded(promise, ms) {
  let timer;
  const limit = new Promise((_, rej) => { timer = setTimeout(() => rej(new Error(`the tool took longer than ${ms} ms`)), ms); });
  return Promise.race([promise, limit]).finally(() => clearTimeout(timer));
}

const ok = (id, result) => ({ jsonrpc: '2.0', id, result });
const fail = (id, code, message) => ({ jsonrpc: '2.0', id, error: { code, message } });

async function handle(msg) {
  if (msg === null || typeof msg !== 'object' || msg.jsonrpc !== '2.0') return fail(null, -32600, 'invalid request');
  const { id, method } = msg;
  const params = msg.params || {};
  if (id === undefined || id === null) return null; // notifications get no answer
  switch (method) {
    case 'initialize':
      return ok(id, { protocolVersion: PROTOCOL, serverInfo: { name: NAME, version: VERSION }, capabilities: { tools: {}, resources: {}, prompts: {} } });
    case 'ping':
      return ok(id, {});
    case 'tools/list':
      return ok(id, { tools: Object.entries(TOOLS).map(([name, t]) => ({ name, description: t.description, inputSchema: t.schema })) });
    case 'tools/call': {
      const tool = Object.prototype.hasOwnProperty.call(TOOLS, params.name) ? TOOLS[params.name] : null;
      if (!tool) return fail(id, -32602, 'unknown tool');
      const args = params.arguments === undefined ? {} : params.arguments;
      try {
        checkArgs(tool.schema, args);
      } catch (e) {
        return fail(id, -32602, e.message);
      }
      try {
        const text = String(await bounded(tool.run(args), TOOL_MS)).slice(0, MAX_RESULT);
        return ok(id, { content: [{ type: 'text', text }], isError: false });
      } catch (e) {
        return ok(id, { content: [{ type: 'text', text: String(e.message) }], isError: true });
      }
    }
    case 'resources/list':
      return ok(id, { resources: Object.entries(RESOURCES).map(([uri, r]) => ({ uri, name: r.name, mimeType: r.mime })) });
    case 'resources/read': {
      const r = RESOURCES[params.uri];
      return r ? ok(id, { contents: [{ uri: params.uri, mimeType: r.mime, text: r.text }] }) : fail(id, -32602, 'unknown resource');
    }
    case 'prompts/list':
      return ok(id, { prompts: Object.entries(PROMPTS).map(([name, p]) => ({ name, description: p.description, arguments: p.arguments })) });
    case 'prompts/get': {
      const p = PROMPTS[params.name];
      if (!p) return fail(id, -32602, 'unknown prompt');
      const text = p.text.replace('{text}', String((params.arguments || {}).text || ''));
      return ok(id, { messages: [{ role: 'user', content: { type: 'text', text } }] });
    }
    default:
      return fail(id, -32601, 'method not found');
  }
}

function serve(input = process.stdin, output = process.stdout) {
  let buf = '';
  let chain = Promise.resolve();
  const line = (l) => {
    chain = chain.then(async () => {
      let out;
      if (l.length > MAX_LINE) out = fail(null, -32600, 'request too large');
      else {
        try { out = await handle(JSON.parse(l)); } catch (e) { out = e instanceof SyntaxError ? fail(null, -32700, 'parse error') : fail(null, -32603, 'internal error'); }
      }
      if (out) output.write(JSON.stringify(out) + '\n');
    });
  };
  input.setEncoding('utf8');
  input.on('data', (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, i); buf = buf.slice(i + 1); if (l.trim()) line(l); }
    if (buf.length > MAX_LINE) buf = '';
  });
}

module.exports = { handle, checkArgs, TOOLS };
// `node server.js` and `node -e <source>` (how a package runs it, where require.main is undefined) both serve; require() does not.
if (require.main === module || require.main === undefined) serve();
