import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';

const ROOT = resolve(import.meta.dirname, '..');
const OUT = join(ROOT, 'tools', 'out');
const PORT = 8143;
const CDP_PORT = 9347;
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

// 伪造 Tauri 桥：popup.js 走 native 分支，事件由测试手动触发。
const MOCK = `(() => {
  const listeners = {};
  window.__themeTest = {
    emit(name, payload) { (listeners[name] || []).forEach(fn => fn({ payload })); },
    saved: { theme: 'light', font_family: 'Segoe UI', sample_interval_ms: 1000 },
  };
  window.__TAURI__ = {
    core: { invoke: async (cmd) => {
      if (cmd === 'get_settings') return { ...window.__themeTest.saved };
      if (cmd === 'get_popup_snapshot') return { history: [], sample: null };
      return null;
    } },
    event: { listen: async (name, fn) => { (listeners[name] ||= []).push(fn); return () => {}; } },
  };
})();`;

const READ = `(() => {
  const cs = (sel, prop) => { const el = document.querySelector(sel); return el ? getComputedStyle(el)[prop] : null; };
  return {
    dataTheme: document.documentElement.dataset.theme || '(未设置)',
    panelBg: cs('.panel', 'backgroundColor'),
    textColor: cs('.heading h2', 'color'),
    status: document.getElementById('status')?.textContent,
  };
})()`;

async function main() {
  await mkdir(OUT, { recursive: true });
  const server = await startServer();
  const chromePath = CHROME_CANDIDATES.find((p) => existsSync(p));
  const profile = join(tmpdir(), 'winmeter-popup-native-' + Date.now());
  const chrome = spawn(chromePath, [
    '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    '--disable-extensions', '--hide-scrollbars',
    `--remote-debugging-port=${CDP_PORT}`, `--user-data-dir=${profile}`,
    '--window-size=1280,570', 'about:blank',
  ], { stdio: 'ignore' });

  let ws;
  const fails = [];
  const check = (label, ok, detail) => {
    console.log(`${ok ? '通过' : '失败'}  ${label}${detail ? '  ' + detail : ''}`);
    if (!ok) fails.push(label);
  };

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
    await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: MOCK });

    await cdp.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/popup.html` });
    await sleep(1500);

    const initial = await cdp.evaluate(READ);
    console.log('初始（已保存=浅色）：', JSON.stringify(initial));
    check('按已保存设置渲染浅色', initial.dataTheme === 'light' && initial.panelBg === 'rgb(251, 252, 254)', initial.dataTheme);

    // 模拟主界面切到深色、设置尚未落盘时广播的预览色
    await cdp.evaluate(`window.__themeTest.emit('theme-preview', { dark: true })`);
    await sleep(250);
    const preview = await cdp.evaluate(READ);
    console.log('收到 theme-preview(dark)：', JSON.stringify(preview));
    check('预览事件把弹窗切成深色', preview.dataTheme === 'dark' && preview.panelBg === 'rgb(25, 26, 32)', preview.dataTheme);
    const shot = await cdp.send('Page.captureScreenshot', { format: 'png' });
    await writeFile(join(OUT, 'popup-native-dark.png'), Buffer.from(shot.data, 'base64'));

    // 再切回浅色，确认双向都跟得上
    await cdp.evaluate(`window.__themeTest.emit('theme-preview', { dark: false })`);
    await sleep(250);
    const back = await cdp.evaluate(READ);
    check('预览事件能切回浅色', back.dataTheme === 'light', back.dataTheme);

    // 保存后仍然走原来的 settings-changed 通道
    await cdp.evaluate(`window.__themeTest.emit('settings-changed', { theme: 'dark', font_family: 'Segoe UI', sample_interval_ms: 1000 })`);
    await sleep(250);
    const saved = await cdp.evaluate(READ);
    check('settings-changed 依旧生效', saved.dataTheme === 'dark', saved.dataTheme);

    console.log('运行时异常：', errors.length ? errors.join(' | ') : '(无)');
    if (errors.length) fails.push('运行时异常');
  } finally {
    try { ws && ws.close(); } catch (e) {}
    chrome.kill();
    server.close();
  }
  if (fails.length) { console.log('未通过：', fails.join('、')); process.exitCode = 1; }
}
main().catch((e) => { console.error('出错：', e); process.exit(1); });
