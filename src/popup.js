import { valid, percent, pct, temperature, bytes, rate, chartPath } from './popup-model.js';
import './settings-io.js';

const $ = id => document.getElementById(id);
const text = (id, value) => { $(id).textContent = value ?? '—'; };
const named = (id, value) => { text(id, value); $(id).title = value ?? ''; };
const gb = value => valid(value) ? `${value.toFixed(1)} GB` : '—';
const native = !!window.__TAURI__?.core?.invoke;
const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
let history = [];
let lastSampleAt = 0;
let interval = 1000;
let settings = { theme: 'dark', font_family: 'Segoe UI' };
const media = matchMedia('(prefers-color-scheme: dark)');

function paint(dark) {
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
}
function applySettings(value) {
  settings = value;
  interval = value.sample_interval_ms || 1000;
  const dark = value.theme === 'dark' || (value.theme === 'system' && media.matches);
  paint(dark);
  if (value.font_family) document.documentElement.style.fontFamily = `"${value.font_family.replace(/["\\]/g, '')}", "Microsoft YaHei UI", sans-serif`;
}
media.addEventListener('change', () => applySettings(settings));
// 主界面把当前生效的深浅色记在 localStorage（预览态也写），弹窗每次重新
// 显示时对一遍，保证「打开就是对的」，不必依赖事件有没有赶上。
function syncCachedTheme() {
  try {
    const cached = localStorage.getItem('winmeter-theme');
    if (cached === 'dark' || cached === 'light') paint(cached === 'dark');
  } catch (e) { /* 读不到就维持现状 */ }
}
window.addEventListener('focus', syncCachedTheme);
document.addEventListener('visibilitychange', () => { if (!document.hidden) syncCachedTheme(); });

function ring(id, value, label) {
  $(id).style.setProperty('--pct', percent(value) ?? 0);
  text(label, pct(value));
}
function transfer(id, value) {
  const formatted = rate(value);
  const [number, unit] = formatted.split(' ');
  $(id).replaceChildren(document.createTextNode(number));
  if (unit) { const small = document.createElement('small'); small.textContent = unit; $(id).append(small); }
}
function chart(id, series, max, now) {
  const svg = $(id);
  svg.replaceChildren();
  for (const [key, color] of series) {
    const paths = chartPath(history, key, max, now);
    for (const [path, fill, opacity, stroke] of [[paths.area, color, .1, 'none'], [paths.line, 'none', 1, color]]) {
      if (!path) continue;
      const element = document.createElementNS('http://www.w3.org/2000/svg', 'path');
      element.setAttribute('d', path); element.setAttribute('fill', fill);
      element.setAttribute('opacity', opacity); element.setAttribute('stroke', stroke);
      element.setAttribute('stroke-width', '1.8'); element.setAttribute('vector-effect', 'non-scaling-stroke');
      svg.append(element);
    }
  }
}
function render(sample, append = true, at = Date.now()) {
  lastSampleAt = at;
  if (append) history.push({ at: lastSampleAt, cpu: sample.cpu_usage, gpu: sample.gpu_usage, memory: sample.mem_load,
    read: sample.disk_read_bps, write: sample.disk_write_bps, down: sample.net_down_bps, up: sample.net_up_bps });
  const now = Date.now();
  history = history.filter(p => p.at >= now - 120000).slice(-121);
  const gpuAvailable = !!sample.gpu_name || !!sample.gpus?.length;
  ring('cpu-ring', sample.cpu_usage, 'cpu-value');
  ring('gpu-ring', gpuAvailable ? sample.gpu_usage : null, 'gpu-value');
  ring('memory-ring', sample.mem_load, 'memory-value');
  named('cpu-name', sample.cpu_name || '处理器信息未提供');
  text('cpu-temp', temperature(sample.cpu_temp_c));
  const physical = sample.sys?.cpu_physical_cores;
  const logical = sample.sys?.cpu_logical_cores ?? sample.core_count;
  text('cpu-cores', `${physical ?? '—'} / ${logical ?? '—'}`);
  text('processes', sample.proc_count);
  named('power-plan', sample.power_plan);
  named('gpu-name', sample.gpu_name || '未检测到显卡');
  text('gpu-temp', temperature(sample.gpu_temp_c));
  text('gpu-temperature', temperature(sample.gpu_temp_c));
  text('gpu-memory', gpuAvailable ? gb(sample.gpu_mem_used_gb) : '—');
  text('gpu-total', gpuAvailable && sample.gpu_mem_total_gb > 0 ? gb(sample.gpu_mem_total_gb) : '—');
  text('memory-total', gb(sample.mem_total_gb));
  text('memory-used', gb(sample.mem_in_use_gb));
  text('memory-standby', gb(sample.mem_standby_gb));
  text('memory-available', gb(sample.mem_available_gb));
  const volumes = sample.volumes || [];
  const volume = volumes.find(v => /^c:/i.test(v.mount)) || volumes[0];
  text('disk-volume', volume?.mount);
  text('disk-free', volume ? `${bytes(volume.free_bytes)} 可用` : '分区信息未提供');
  const used = volume?.total_bytes > 0 && valid(volume.free_bytes) ? percent((1 - volume.free_bytes / volume.total_bytes) * 100) : null;
  $('disk-capacity').style.width = `${used ?? 0}%`;
  text('disk-used', used == null ? '—' : `${Math.round(used)}% 已使用`);
  text('disk-total', volume ? bytes(volume.total_bytes) : '—');
  transfer('disk-read', sample.disk_read_bps); transfer('disk-write', sample.disk_write_bps);
  text('net-kind', sample.net?.kind);
  named('net-ip', sample.net?.local_ip || '未连接网络');
  transfer('net-up', sample.net_up_bps); transfer('net-down', sample.net_down_bps);
  text('net-sent', bytes(sample.net_session_up_bytes)); text('net-received', bytes(sample.net_session_down_bytes));
  const battery = sample.battery;
  const present = !!battery?.present;
  text('battery-state', present ? '电池' : '电源');
  text('battery-value', present ? pct(battery.percent) : '—');
  $('battery-level').style.width = `${present ? percent(battery.percent) ?? 0 : 0}%`;
  text('battery-note', present ? (battery.charging ? '正在充电' : battery.ac_line ? '已连接电源' : '电池供电') : '未检测到电池');
  text('sensor-cpu', temperature(sample.cpu_temp_c)); text('sensor-gpu', temperature(sample.gpu_temp_c));
  text('sensor-disk', temperature(sample.disk_temp_c));
  text('sensor-fan', valid(sample.fan_rpm) ? `${Math.round(sample.fan_rpm)} RPM` : '—');
  text('sensor-note', [sample.cpu_temp_c, sample.gpu_temp_c, sample.disk_temp_c].some(valid)
    ? '未提供的读数显示 —' : '温度未获取，可在设置中启用采集');
  chart('cpu-chart', [['cpu', 'var(--cyan)']], 100, now);
  chart('gpu-chart', [['gpu', 'var(--cyan)']], 100, now);
  chart('memory-chart', [['memory', 'var(--purple)']], 100, now);
  const peak = keys => Math.max(1024, ...history.flatMap(p => keys.map(k => valid(p[k]) ? p[k] : 0)));
  chart('disk-chart', [['read', 'var(--cyan)'], ['write', 'var(--pink)']], peak(['read', 'write']), now);
  chart('net-chart', [['up', 'var(--amber)'], ['down', 'var(--green)']], peak(['up', 'down']), now);
  const stale = native && now - lastSampleAt > Math.max(10000, interval * 3);
  text('status', native ? (stale ? '采样暂时中断' : '实时采集中') : '演示数据');
  $('status-dot').className = native && !stale ? 'live' : '';
  text('updated', `${native ? '更新于' : '浏览器预览 ·'} ${new Date(lastSampleAt).toLocaleTimeString('zh-CN', { hour12: false })}`);
}

async function close() {
  if (native) await invoke('close_popup');
  else text('status', '预览模式 · 程序内可按 Esc 收起');
}
$('close').addEventListener('click', () => close().catch(() => text('status', '收起失败，请重试')));
document.addEventListener('keydown', event => { if (event.key === 'Escape') close().catch(() => {}); });
document.querySelectorAll('[data-page]').forEach(button => button.addEventListener('click', async () => {
  try {
    if (native) await invoke('open_main', { page: button.dataset.page });
    else text('status', '浏览器预览 · 程序内打开对应详情');
  } catch { text('status', '打开详情失败，请重试'); }
}));

async function start() {
  if (!native) {
    applySettings({ theme: new URLSearchParams(location.search).get('theme') === 'light' ? 'light' : 'dark' });
    // Browser-only sample data, visibly labeled. Native mode never falls back to it.
    const demo = () => ({
      cpu_usage: 28 + Math.random() * 8, cpu_name: 'Intel Core Ultra 7 255HX', cpu_temp_c: 54,
      core_count: 20, sys: { cpu_physical_cores: 20, cpu_logical_cores: 20 }, proc_count: 238, power_plan: '平衡',
      gpu_name: 'NVIDIA GeForce RTX 5060 Laptop GPU', gpu_usage: 13 + Math.random() * 6,
      gpu_temp_c: 54, gpu_mem_used_gb: 1.03, gpu_mem_total_gb: 8,
      mem_load: 47, mem_total_gb: 32, mem_in_use_gb: 15.04, mem_standby_gb: 8.2, mem_available_gb: 16.96,
      volumes: [{ mount: 'C:', total_bytes: 1024 ** 4, free_bytes: 556.86 * 1024 ** 3 }],
      disk_read_bps: (100 + Math.random() * 180) * 1024, disk_write_bps: (300 + Math.random() * 400) * 1024,
      net: { local_ip: '192.168.18.135', kind: 'Wi-Fi' }, net_up_bps: 5.7 * 1024, net_down_bps: 4.8 * 1024,
      net_session_up_bytes: 800.43 * 1024 ** 2, net_session_down_bytes: 1.28 * 1024 ** 3,
      battery: { present: true, charging: false, percent: 80 }, disk_temp_c: 41, fan_rpm: 1234,
    });
    const now = Date.now();
    history = Array.from({ length: 120 }, (_, i) => ({ at: now - (120 - i) * 1000,
      cpu: 22 + Math.random() * 16, gpu: 8 + Math.random() * 28, memory: 45 + Math.random() * 3,
      read: Math.random() * 400000, write: Math.random() * 600000,
      up: i > 65 ? 5000 : Math.random() * 25000, down: i > 65 ? 4000 : Math.random() * 18000 }));
    render(demo()); setInterval(() => render(demo()), 1000);
    return;
  }
  let loading = true;
  const pending = [];
  try {
    await window.__TAURI__.event.listen('sample', event => {
      if (loading) pending.push(event.payload); else render(event.payload);
    });
    await window.__TAURI__.event.listen('settings-changed', event => applySettings(event.payload));
    // 主界面切主题时先广播预览色，弹窗不必等设置落盘就能跟着变。
    await window.__TAURI__.event.listen('theme-preview', event => {
      if (typeof event.payload?.dark === 'boolean') paint(event.payload.dark);
    });
    applySettings(await window.WinMeterSettingsIO.load(invoke));
    const snapshot = await invoke('get_popup_snapshot');
    history = snapshot.history || [];
    if (snapshot.sample) render(snapshot.sample, false, history.at(-1)?.at ?? Date.now());
  } catch {
    text('status', '连接失败，请重新打开 WinMeter');
  }
  loading = false;
  for (const sample of pending) render(sample);
  setInterval(() => {
    if (lastSampleAt && Date.now() - lastSampleAt > Math.max(10000, interval * 3)) {
      text('status', '采样暂时中断'); $('status-dot').className = '';
    }
  }, 2000);
}
start();
