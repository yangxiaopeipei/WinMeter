import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const ROOT = resolve(import.meta.dirname, '..');
const OUT = join(ROOT, 'tools', 'out');
const PORT = 8141;
const CDP_PORT = 9345;
const CHROME_CANDIDATES = [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
];
const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.json': 'application/json; charset=utf-8', '.png': 'image/png', '.svg': 'image/svg+xml' };

function startServer() {
  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url, 'http://localhost');
      let p = decodeURIComponent(url.pathname);
      if (p === '/') p = '/popup.html';
      const file = join(ROOT, 'src', p);
      if (!file.startsWith(ROOT) || !existsSync(file)) return res.writeHead(404).end('not found');
      const buf = await readFile(file);
      res.writeHead(200, { 'Content-Type': MIME[extname(file)] || 'application/octet-stream' });
      res.end(buf);
    } catch (e) { res.writeHead(500).end(String(e)); }
  });
  return new Promise((r) => server.listen(PORT, '127.0.0.1', () => r(server)));
}

class CDP {
  constructor(ws) {
    this.ws = ws; this.id = 0; this.pending = new Map();
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? reject(new Error(JSON.stringify(msg.error))) : resolve(msg.result);
      }
    });
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => { if (this.pending.has(id)) { this.pending.delete(id); reject(new Error('CDP timeout: ' + method)); } }, 30000);
    });
  }
  async evaluate(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || JSON.stringify(r.exceptionDetails));
    return r.result.value;
  }
}

async function waitForCdp() {
  for (let i = 0; i < 60; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`);
      const list = await r.json();
      const page = list.find((t) => t.type === 'page');
      if (page) return page;
    } catch (e) {}
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error('Chrome 调试端口未就绪');
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const READ = `(() => {
  const cs = (sel, prop) => { const el = document.querySelector(sel); return el ? getComputedStyle(el)[prop] : null; };
  return {
    dataTheme: document.documentElement.dataset.theme || '(未设置)',
    colorScheme: cs('html', 'colorScheme'),
    bgVar: getComputedStyle(document.documentElement).getPropertyValue('--bg').trim(),
    textVar: getComputedStyle(document.documentElement).getPropertyValue('--text').trim(),
    panelBg: cs('.panel', 'backgroundColor'),
    panelColor: cs('.panel', 'color'),
    gaugeInner: cs('.gauge > div', 'backgroundColor'),
    h2Color: cs('.heading h2', 'color'),
    muted: cs('.device', 'color'),
    footerColor: cs('footer', 'color'),
  };
})()`;

async function main() {
  await mkdir(OUT, { recursive: true });
  const server = await startServer();
  const chromePath = CHROME_CANDIDATES.find((p) => existsSync(p));
  const profile = join(tmpdir(), 'winmeter-popup-' + Date.now());
  const chrome = spawn(chromePath, [
    '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    '--disable-extensions', '--hide-scrollbars',
    `--remote-debugging-port=${CDP_PORT}`, `--user-data-dir=${profile}`,
    '--window-size=1280,570', 'about:blank',
  ], { stdio: 'ignore' });

  let ws;
  try {
    const page = await waitForCdp();
    ws = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((res, rej) => { ws.addEventListener('open', res); ws.addEventListener('error', rej); });
    const cdp = new CDP(ws);
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    const errors = [];
    ws.addEventListener('message', (ev) => {
      const m = JSON.parse(ev.data);
      if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
    });
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 570, deviceScaleFactor: 1, mobile: false });

    for (const t of ['dark', 'light']) {
      await cdp.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/popup.html?theme=${t}` });
      await sleep(1400);
      const state = await cdp.evaluate(READ);
      console.log(`===== ?theme=${t} =====`);
      console.log(JSON.stringify(state, null, 2));
      const shot = await cdp.send('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(OUT, `popup-${t}.png`), Buffer.from(shot.data, 'base64'));
    }
    console.log('===== 异常 =====');
    console.log(errors.length ? errors.join('\n') : '(无)');
  } finally {
    try { ws && ws.close(); } catch (e) {}
    chrome.kill();
    server.close();
  }
}
main().catch((e) => { console.error('出错：', e); process.exit(1); });
