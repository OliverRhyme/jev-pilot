// What Chrome answers when a row read from the page is replaced by an identical
// copy before it is clicked, as GitHub's live update does to its star button.
// usage: node probe.mjs <chrome binary> <out dir>
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, existsSync, writeFileSync, rmSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import http from 'node:http';

const [chrome, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const profile = mkdtempSync(join(process.env.SCRATCH || tmpdir(), 'profile-'));
const page = `<!doctype html><html><head><title>Repo</title></head><body style="margin:0">
<div id="box"><button id="star" style="position:absolute;left:100px;top:100px;width:120px;height:32px" aria-label="Unstar this repository"><span>Starred</span> <span class="Counter">2</span></button></div>
</body></html>`;
const server = http.createServer((req, res) => { res.setHeader('content-type', 'text/html'); res.end(page); });
await new Promise(r => server.listen(0, '127.0.0.1', r));
const url = `http://127.0.0.1:${server.address().port}/repo`;
const proc = spawn(chrome, ['--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run',
  '--no-default-browser-check', '--headless', '--window-size=1280,800', 'about:blank'], { stdio: 'ignore' });
const portFile = join(profile, 'DevToolsActivePort');
for (let i = 0; i < 100 && !existsSync(portFile); i++) await new Promise(r => setTimeout(r, 100));
const [port, path] = readFileSync(portFile, 'utf8').split('\n');
const ws = new WebSocket(`ws://127.0.0.1:${port}${path}`);
await new Promise(r => ws.addEventListener('open', r));
let id = 0; const pending = new Map();
ws.addEventListener('message', m => { const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); } });
const call = (method, params = {}, sessionId) => new Promise(resolve => {
  const i = ++id; pending.set(i, resolve); ws.send(JSON.stringify({ id: i, method, params, sessionId })); });
const save = (name, value) => writeFileSync(join(out, name), JSON.stringify(value, null, 1) + '\n');
try {
  const created = (await call('Target.createTarget', { url: 'about:blank' })).result;
  const session = (await call('Target.attachToTarget', { targetId: created.targetId, flatten: true })).result.sessionId;
  await call('Page.enable', {}, session);
  await call('Page.navigate', { url }, session);
  await new Promise(r => setTimeout(r, 1000));
  const at = { x: 160, y: 116, includeUserAgentShadowDOM: false, ignorePointerEventsNone: true };
  const before = await call('DOM.getNodeForLocation', at, session);
  save('hit-before.json', before);
  const old = before.result.backendNodeId;
  await call('Runtime.evaluate', { expression: "document.getElementById('box').innerHTML = document.getElementById('box').innerHTML" }, session);
  await call('HeapProfiler.collectGarbage', {}, session);
  const after = await call('DOM.getNodeForLocation', at, session);
  save('hit-after.json', after);
  save('resolve-replaced.json', await call('DOM.resolveNode', { backendNodeId: old }, session));
  save('resolve-current.json', await call('DOM.resolveNode', { backendNodeId: after.result.backendNodeId }, session));
  save('ax-partial.json', await call('Accessibility.getPartialAXTree', { backendNodeId: after.result.backendNodeId, fetchRelatives: true }, session));
} finally {
  try { await call('Browser.close'); } catch {}
  ws.close(); server.close(); proc.kill();
  await new Promise(r => setTimeout(r, 500));
  rmSync(profile, { recursive: true, force: true });
}
