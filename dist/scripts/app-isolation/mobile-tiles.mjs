// The browser half of the app isolation check (docs/development/APP-ISOLATION-CHECK.md): the web UI in a phone-sized window.
// Usage: node mobile-tiles.mjs BASE_URL USER PASSWORD OUT_DIR   (BASE_URL like https://127.0.0.1:8479)
// Prints one line per check, "PASS ..." or "FAIL ...", and exits 1 on any FAIL. Screenshots go to OUT_DIR.
import { launch } from './cdp.mjs';
const [base, user, pass, out] = process.argv.slice(2);
const W = 390, H = 844;
const b = await launch();
let bad = 0;
const say = (ok, msg) => { console.log((ok ? 'PASS ' : 'FAIL ') + msg); if (!ok) bad++; };
await b.send('Page.enable');
await b.send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: 2, mobile: true });
await b.send('Emulation.setTouchEmulationEnabled', { enabled: true });
await b.send('Network.setUserAgentOverride', { userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148' });
await b.send('Page.navigate', { url: base + '/ui/' });
await b.wait(2000);
await b.ev(`document.querySelector('#user').value=${JSON.stringify(user)}; document.querySelector('#pass').value=${JSON.stringify(pass)}; document.querySelector('#signin-form').requestSubmit();`);
await b.wait(2000);
const hdr = await b.ev(`Math.round(document.querySelector('#top').getBoundingClientRect().height)`);
say(hdr <= H * 0.4, `the header takes ${hdr} px of ${H} (limit 40%)`);
const broken = await b.ev(`[...document.querySelectorAll('#nav button')].filter(x=>x.getBoundingClientRect().height > 60).map(x=>x.textContent.trim()).join(',')`);
say(!broken, `no page name is broken over several lines${broken ? ' (' + broken + ')' : ''}`);
await b.shot(out + '/home.png');
await b.ev(`[...document.querySelectorAll('#nav button')].find(x=>x.textContent.trim()==='Tiles').click()`);
await b.wait(2000);
const names = (await b.ev(`[...document.querySelectorAll('#page button')].map(x=>x.textContent.trim()).filter(t=>t&&t!=='Reload')`)) || [];
say(names.length > 0, `the Tiles page lists ${names.length} tiles`);
for (const name of names) {
  await b.ev(`[...document.querySelectorAll('#page button')].find(x=>x.textContent.trim()===${JSON.stringify(name)}).click()`);
  await b.wait(3500);
  const r = JSON.parse(await b.ev(`(()=>{const c=document.querySelector('#page canvas'); if(!c) return '{"none":true}'; const q=c.getBoundingClientRect(); return JSON.stringify({l:q.left,r:q.right,sw:document.documentElement.scrollWidth})})()`));
  say(!r.none && r.l >= 0 && r.r <= W + 1, `tile "${name}" fits inside ${W} px (${r.none ? 'no canvas' : Math.round(r.l) + '..' + Math.round(r.r)})`);
  say(!r.none && r.sw <= W, `tile "${name}" adds no horizontal scroll (${r.sw} px)`);
  await b.shot(out + '/tile-' + name.split(' ')[0].toLowerCase() + '.png');
}
b.close();
process.exit(bad ? 1 : 0);
