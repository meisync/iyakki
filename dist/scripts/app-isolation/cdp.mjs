// minimal Chrome DevTools client for the isolation check (no dependencies, node 22)
import { spawn } from 'node:child_process';
import { writeFileSync } from 'node:fs';
const PORT = Number(process.env.CDP_PORT || 9333);
export async function launch() {
  const ch = spawn('google-chrome', ['--headless=new', '--no-sandbox', '--disable-gpu', '--ignore-certificate-errors', `--remote-debugging-port=${PORT}`, `--user-data-dir=${process.env.CDP_PROFILE || '/tmp/iyakki-iso-chrome'}`, 'about:blank'], { stdio: 'ignore' });
  let info;
  for (let i = 0; i < 60; i++) { try { info = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json(); if (info.length) break; } catch {} await new Promise(r => setTimeout(r, 250)); }
  const page = info.find(t => t.type === 'page');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise(r => ws.addEventListener('open', r));
  let id = 0; const pend = new Map();
  ws.addEventListener('message', ev => { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
  const send = (method, params = {}) => new Promise(res => { const i = ++id; pend.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
  const ev = async (expr) => { const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true }); return r.result?.result?.value; };
  const shot = async (file) => { const r = await send('Page.captureScreenshot', { format: 'png' }); writeFileSync(file, Buffer.from(r.result.data, 'base64')); };
  const wait = ms => new Promise(r => setTimeout(r, ms));
  return { send, ev, shot, wait, close: () => { try { ws.close(); } catch {} ch.kill(); } };
}
