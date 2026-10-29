import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const ROOT = resolve(import.meta.dirname, '..');
const OUT = join(ROOT, 'tools', 'out');
const PORT = 8139;
const CDP_PORT = 9343;

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
const PAGES = ['overview', 'cpu', 'gpu', 'mem', 'net', 'disk', 'battery', 'bt', 'info', 'settings'];

async function main() {
  await mkdir(OUT, { recursive: true });
  const server = await startServer();
  const chromePath = CHROME_CANDIDATES.find((p) => existsSync(p));
  const profile = join(tmpdir(), 'winmeter-pages-' + Date.now());
  const chrome = spawn(chromePath, [
    '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    '--disable-extensions', '--hide-scrollbars',
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

    const errors = [];
    ws.addEventListener('message', (ev) => {
      const m = JSON.parse(ev.data);
      if (m.method === 'Runtime.exceptionThrown') {
        errors.push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
      }
    });

    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1120, height: 740, deviceScaleFactor: 1, mobile: false });
    await cdp.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/index.html` });
    await sleep(1800);

    const rows = [];
    for (const p of PAGES) {
      // 切页：点击侧栏，goPage 应该立刻用最近一帧补画
      const r = await cdp.evaluate(`(() => {
        const ni = document.querySelector('#side .ni[data-p="${p}"]');
        if (ni) ni.click();
        const pg = document.querySelector('.page[data-p="${p}"]');
        if (!pg) return { page: '${p}', missing: true };
        // 统计该页可见文本里有多少个占位符「—」、多少个真实读数
        const leaves = [...pg.querySelectorAll('*')].filter(el => el.children.length === 0 && el.textContent.trim());
        const dash = leaves.filter(el => el.textContent.trim() === '—').length;
        const filled = leaves.length - dash;
        const svgPaths = [...pg.querySelectorAll('svg path')].filter(x => (x.getAttribute('d')||'').length > 8).length;
        const tableRows = pg.querySelectorAll('tbody tr').length;
        return { page: '${p}', leaves: leaves.length, filled, dash, svgPaths, tableRows };
      })()`);
      rows.push(r);
    }

    // 测一次完整渲染耗时（demo 模式每秒一帧）
    const perf = await cdp.evaluate(`(async () => {
      const t = [];
      for (let i = 0; i < 6; i++) {
        const t0 = performance.now();
        await new Promise(r => requestAnimationFrame(r));
        t.push(performance.now() - t0);
      }
      // 直接量一帧渲染：切到概览再切回，取 goPage 同步补画耗时
      const out = [];
      for (const p of ['overview','cpu','mem','disk']) {
        const ni = document.querySelector('#side .ni[data-p="' + p + '"]');
        const t0 = performance.now();
        if (ni) ni.click();
        out.push([p, +(performance.now() - t0).toFixed(2)]);
      }
      return { frameMs: t.map(x => +x.toFixed(1)), pageRepaintMs: out };
    })()`);

    // 根节点底色：body 曾经因为拿不到 .win 上的变量而变成透明，露出 WebView2 黑画布
    const bg = await cdp.evaluate(`(async () => {
      const read = () => ({
        html: getComputedStyle(document.documentElement).backgroundColor,
        body: getComputedStyle(document.body).backgroundColor,
        win: getComputedStyle(document.querySelector('.win')).backgroundColor,
      });
      const light = read();
      document.getElementById('btnTheme').click();
      await new Promise(r => setTimeout(r, 120));
      const dark = read();
      const stored = localStorage.getItem('winmeter-theme');
      document.getElementById('btnTheme').click();
      await new Promise(r => setTimeout(r, 120));
      return { light, dark, stored };
    })()`);
    console.log('===== 根节点底色 =====');
    console.log(JSON.stringify(bg));

    await cdp.evaluate(`document.querySelector('#side .ni[data-p="overview"]').click()`);
    await sleep(300);
    const shot = await cdp.send('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(OUT, 'pages-overview.png'), Buffer.from(shot.data, 'base64'));

    await cdp.evaluate(`document.querySelector('#side .ni[data-p="cpu"]').click()`);
    await sleep(300);
    const shot2 = await cdp.send('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(OUT, 'pages-cpu.png'), Buffer.from(shot2.data, 'base64'));

    console.log('===== 各页填充情况 =====');
    for (const r of rows) console.log(JSON.stringify(r));
    console.log('===== 渲染耗时 =====');
    console.log(JSON.stringify(perf));
    console.log('===== 运行时异常 =====');
    console.log(errors.length ? errors.join('\n') : '(无)');

    const empty = rows.filter((r) => !r.missing && r.filled < 4 && r.page !== 'bt');
    if (empty.length) { console.log('疑似未渲染：', JSON.stringify(empty)); process.exitCode = 1; }
    if (errors.length) process.exitCode = 1;
  } finally {
    try { ws && ws.close(); } catch (e) { /* ignore */ }
    chrome.kill();
    server.close();
  }
}

main().catch((e) => { console.error('出错：', e); process.exit(1); });
