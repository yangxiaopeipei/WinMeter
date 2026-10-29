import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const ROOT = resolve(import.meta.dirname, '..');
const OUT = join(ROOT, 'tools', 'out');
const PORT = 8137;
const CDP_PORT = 9341;

const CHROME_CANDIDATES = [
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
];
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
};

function startServer() {
  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url, 'http://localhost');
      let p = decodeURIComponent(url.pathname);
      if (p === '/') p = '/index.html';
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
    } catch (e) { /* retry */ }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error('Chrome 调试端口未就绪');
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  await mkdir(OUT, { recursive: true });
  const server = await startServer();
  const chromePath = CHROME_CANDIDATES.find((p) => existsSync(p));
  if (!chromePath) throw new Error('未找到 Chrome / Edge');
  const profile = join(tmpdir(), 'winmeter-mem-' + Date.now());
  const chrome = spawn(chromePath, [
    '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    '--disable-extensions', '--hide-scrollbars', '--force-device-scale-factor=2',
    `--remote-debugging-port=${CDP_PORT}`, `--user-data-dir=${profile}`,
    '--window-size=1120,740', 'about:blank',
  ], { stdio: 'ignore' });

  let ws;
  try {
    const page = await waitForCdp();
    ws = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((res, rej) => { ws.addEventListener('open', res); ws.addEventListener('error', rej); });
    const cdp = new CDP(ws);
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');

    const logs = [];
    ws.addEventListener('message', (ev) => {
      const m = JSON.parse(ev.data);
      if (m.method === 'Runtime.consoleAPICalled') logs.push(m.params.args.map((a) => a.value ?? a.description ?? '').join(' '));
    });

    const views = [
      { name: 'mem-1120-light', w: 1120, h: 740, dark: false },
      { name: 'mem-1120-dark', w: 1120, h: 740, dark: true },
      { name: 'mem-900-light', w: 900, h: 620, dark: false },
    ];

    const report = [];
    for (const v of views) {
      await cdp.send('Emulation.setDeviceMetricsOverride', { width: v.w, height: v.h, deviceScaleFactor: 2, mobile: false });
      await cdp.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html?p=mem${v.dark ? '&t=dark' : ''}` });
      await sleep(1600);

      const r = await cdp.evaluate(`(() => {
        // 主题会从本地存储恢复，先按目标强制一次，截图才有可比性
        const win = document.querySelector('.win');
        const want = ${v.dark ? "'dark'" : "'light'"};
        if (win && win.dataset.t !== want) document.getElementById('btnTheme').click();
        const t = (id) => { const e = document.getElementById(id); return e ? e.textContent : null; };
        const cap = document.getElementById('mem-cap');
        const row = document.querySelector('[data-p="mem"] .ttl');
        return {
          pill: t('mem-bm'),
          cap: cap ? cap.textContent : null,
          capUsedPx: cap ? parseFloat(getComputedStyle(cap).fontSize) : null,
          suffixPx: cap && cap.querySelector('.u') ? parseFloat(getComputedStyle(cap.querySelector('.u')).fontSize) : null,
          spec: t('mem-bs'),
          specPx: document.getElementById('mem-bs') ? parseFloat(getComputedStyle(document.getElementById('mem-bs')).fontSize) : null,
          rowH: row ? Math.round(row.getBoundingClientRect().height) : null,
          overflow: row ? row.scrollWidth > row.clientWidth + 1 : null,
          infoSpec: (() => {
            const rows = [...document.querySelectorAll('#info-hw-body tr')];
            const hit = rows.find((tr) => tr.textContent.startsWith('内存规格'));
            return hit ? hit.textContent : null;
          })(),
        };
      })()`);
      const shot = await cdp.send('Page.captureScreenshot', { format: 'png' });
      const file = join(OUT, v.name + '.png');
      await writeFile(file, Buffer.from(shot.data, 'base64'));
      report.push({ view: v.name, ...r, file: file.replace(ROOT, '.') });
    }

    console.log('===== 结果 =====');
    for (const r of report) console.log(JSON.stringify(r, null, 0));
    const bad = report.filter((r) => r.overflow || !r.cap || r.cap === '—');
    if (bad.length) { console.log('异常视图：', JSON.stringify(bad, null, 2)); process.exitCode = 1; }
    console.log('控制台：', logs.slice(-8).join(' | ') || '(无)');
  } finally {
    try { ws && ws.close(); } catch (e) { /* ignore */ }
    chrome.kill();
    server.close();
  }
}

main().catch((e) => { console.error('出错：', e); process.exit(1); });
