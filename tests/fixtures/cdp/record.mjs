// Records raw CDP replies from a throwaway Chrome for Testing.
// usage: node record.mjs <chrome binary> <out dir>
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, existsSync, writeFileSync, rmSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import http from 'node:http';

const [chrome, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const profile = mkdtempSync(join(process.env.SCRATCH || tmpdir(), 'profile-'));

const page = `<!doctype html><html><head><title>Sign up</title><style>
body{margin:0;font:16px sans-serif} header{position:sticky;top:0;background:#eee;height:48px}
.spacer{height:900px} .card{padding:8px;cursor:pointer;border:1px solid #ccc;width:200px}
</style></head><body>
<header><nav aria-label="Main"><a href="/home">Home</a> <a href="/help">Help</a></nav></header>
<main><h1>Create your account</h1>
<form onsubmit="return false">
<label>Email <input id="email" type="email" placeholder="you@example.com"></label>
<label>Password <input type="password" value="hunter2"></label>
<label>Country <select id="country"><option>Japan</option><option selected>Philippines</option><option>Spain</option></select></label>
<label><input type="checkbox" checked> Send me news</label>
<button type="submit" disabled>Create account</button>
</form>
<div class="card" id="card">Pick the free plan</div>
<iframe id="frame" srcdoc="<button onclick='1'>Inside frame</button><p>Frame text</p>" style="width:300px;height:80px"></iframe>
<div class="spacer"></div>
<h2>Further down</h2><a href="/terms">Terms of service</a>
</main>
<script>document.getElementById('card').addEventListener('click',()=>{})</script>
</body></html>`;

const server = http.createServer((req, res) => { res.setHeader('content-type', 'text/html'); res.end(page); });
await new Promise(r => server.listen(0, '127.0.0.1', r));
const url = `http://127.0.0.1:${server.address().port}/signup`;

const proc = spawn(chrome, ['--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run',
  '--no-default-browser-check', '--headless', '--window-size=1280,800', 'about:blank'], { stdio: 'ignore' });
let portFile = join(profile, 'DevToolsActivePort');
for (let i = 0; i < 100 && !existsSync(portFile); i++) await new Promise(r => setTimeout(r, 100));
const [port, path] = readFileSync(portFile, 'utf8').split('\n');
const ws = new WebSocket(`ws://127.0.0.1:${port}${path}`);
await new Promise(r => ws.addEventListener('open', r));
let id = 0; const pending = new Map(); const events = [];
ws.addEventListener('message', m => { const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); } else events.push(msg); });
const call = (method, params = {}, sessionId, timeout = 5000) => new Promise(resolve => {
  const i = ++id; pending.set(i, resolve); ws.send(JSON.stringify({ id: i, method, params, sessionId }));
  setTimeout(() => { if (pending.has(i)) { pending.delete(i); resolve({ timeout: true }); } }, timeout); });
const save = (name, value) => writeFileSync(join(out, name), JSON.stringify(value, null, 1) + '\n');

try {
  const version = (await call('Browser.getVersion')).result;
  save('version.json', version);
  await call('Target.setDiscoverTargets', { discover: true });
  const created = (await call('Target.createTarget', { url: 'about:blank' })).result;
  const session = (await call('Target.attachToTarget', { targetId: created.targetId, flatten: true })).result.sessionId;
  await call('Page.enable', {}, session);
  await call('Page.navigate', { url }, session);
  await new Promise(r => setTimeout(r, 1500));
  await call('Runtime.evaluate', { expression: 'window.scrollTo(0, 120)' }, session);
  await new Promise(r => setTimeout(r, 300));
  const tree = (await call('Page.getFrameTree', {}, session)).result;
  save('frame-tree.json', tree);
  const frames = [tree.frameTree.frame, ...(tree.frameTree.childFrames || []).map(c => c.frame)];
  for (const [i, f] of frames.entries()) {
    save(`ax-${i}.json`, (await call('Accessibility.getFullAXTree', { frameId: f.id }, session)).result);
  }
  save('snapshot.json', (await call('DOMSnapshot.captureSnapshot', { computedStyles: [], includePaintOrder: true, includeDOMRects: true }, session)).result);
  save('layout-metrics.json', (await call('Page.getLayoutMetrics', {}, session)).result);
  save('targets.json', (await call('Target.getTargets')).result);
  // hit test the card, and the box model of the email field
  const loc = (await call('DOM.getNodeForLocation', { x: 50, y: 300, includeUserAgentShadowDOM: false, ignorePointerEventsNone: true }, session));
  save('node-for-location.json', loc);
  // dialogs: does an open confirm hang the reads?
  await call('Runtime.evaluate', { expression: 'setTimeout(() => confirm("Really delete?"), 0)' }, session);
  await new Promise(r => setTimeout(r, 500));
  const notes = {};
  notes.dialogEvent = events.find(e => e.method === 'Page.javascriptDialogOpening') || null;
  for (const [m, p] of [['Page.getFrameTree', {}], ['Accessibility.getFullAXTree', {}], ['DOMSnapshot.captureSnapshot', { computedStyles: [] }], ['Page.getLayoutMetrics', {}], ['DOM.getNodeForLocation', { x: 50, y: 300 }]]) {
    const t0 = Date.now(); const r = await call(m, p, session, 3000);
    notes[m] = r.timeout ? 'timed out after 3s' : (r.error ? `error ${r.error.message}` : `answered in ${Date.now() - t0}ms`);
  }
  await call('Page.handleJavaScriptDialog', { accept: false }, session);
  save('dialog-notes.json', notes);
  save('events.json', events.filter(e => e.method.startsWith('Target.') || e.method.startsWith('Page.javascript')));
} finally {
  try { await call('Browser.close', {}, undefined, 2000); } catch {}
  ws.close(); server.close(); proc.kill();
  await new Promise(r => setTimeout(r, 500));
  rmSync(profile, { recursive: true, force: true });
}
