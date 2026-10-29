// WinMeter 前端：订阅 Tauri 的 sample 事件并实时渲染系统指标。
(function () {
  'use strict';

  const $ = (id) => document.getElementById(id);
  const set = (id, v) => { const el = $(id); if (el) el.textContent = v; };
  // 少量需要混排字号的读数（例如容量的单位）只能走 innerHTML，
  // 调用点传的都是本地拼好的数字，不含外部字符串。
  const setHtml = (id, v) => { const el = $(id); if (el) el.innerHTML = v; };

  // ---------- 格式化 ----------
  function fmtBytes(bytes) {
    if (bytes == null || isNaN(bytes)) return '—';
    const b = Number(bytes);
    if (b >= 1024 ** 4) return (b / 1024 ** 4).toFixed(1) + ' TB';
    if (b >= 1024 ** 3) return (b / 1024 ** 3).toFixed(1) + ' GB';
    if (b >= 1024 ** 2) return (b / 1024 ** 2).toFixed(0) + ' MB';
    if (b >= 1024) return (b / 1024).toFixed(0) + ' KB';
    return b.toFixed(0) + ' B';
  }
  function fmtGB(gb) {
    return gb == null || isNaN(gb) ? '—' : gb.toFixed(1) + ' GB';
  }
  // 两个容量用同一单位，避免「998 GB / 2.50 TB」
  function fmtPair(used, total) {
    const T = 1024 ** 4, G = 1024 ** 3;
    if (total >= T) return (used / T).toFixed(2) + ' / ' + (total / T).toFixed(2) + ' TB';
    return Math.round(used / G) + ' / ' + Math.round(total / G) + ' GB';
  }
  // 单个容量（整数 GB，避免 384.0 GB 这种噪点）
  function fmtCap(bytes) {
    if (bytes == null || isNaN(bytes)) return '—';
    const b = Number(bytes);
    if (b >= 1024 ** 4) return (b / 1024 ** 4).toFixed(2) + ' TB';
    if (b >= 1024 ** 3) return (b / 1024 ** 3).toFixed(0) + ' GB';
    if (b >= 1024 ** 2) return (b / 1024 ** 2).toFixed(0) + ' MB';
    return (b / 1024).toFixed(0) + ' KB';
  }
  function fmtRate(bps) {
    if (bps == null || isNaN(bps) || bps < 0.5) return '0 B/s';
    const b = Number(bps);
    if (b >= 1024 ** 3) return (b / 1024 ** 3).toFixed(2) + ' GB/s';
    if (b >= 1024 ** 2) return (b / 1024 ** 2).toFixed(1) + ' MB/s';
    if (b >= 1024) return (b / 1024).toFixed(0) + ' KB/s';
    return b.toFixed(0) + ' B/s';
  }
  // 侧栏用的紧凑写法：2.5M / 164K
  function fmtRateTiny(bps) {
    if (bps == null || isNaN(bps) || bps < 0.5) return '0';
    const b = Number(bps);
    if (b >= 1024 ** 3) return (b / 1024 ** 3).toFixed(1) + 'G';
    if (b >= 1024 ** 2) return (b / 1024 ** 2).toFixed(1) + 'M';
    if (b >= 1024) return (b / 1024).toFixed(0) + 'K';
    return b.toFixed(0) + 'B';
  }
  function fmtPct(v) {
    return v == null || isNaN(v) ? '—' : v.toFixed(0) + '%';
  }
  function fmtDuration(sec) {
    if (sec == null || isNaN(sec)) return '—';
    const s = Math.floor(sec);
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    const ss = s % 60;
    const p2 = (n) => String(n).padStart(2, '0');
    return (d > 0 ? d + '天 ' : '') + p2(h) + ':' + p2(m) + ':' + p2(ss);
  }
  // 3 小时 42 分 → 3:42
  function fmtHM(sec) {
    if (sec == null || isNaN(sec)) return '—';
    const m = Math.round(sec / 60);
    if (m < 60) return m + ' 分';
    return Math.floor(m / 60) + ':' + String(m % 60).padStart(2, '0');
  }
  function fmtAgo(sec) {
    if (sec == null || isNaN(sec)) return '未提供';
    if (sec < 60) return '刚刚';
    if (sec < 3600) return Math.floor(sec / 60) + ' 分钟前';
    if (sec < 86400) return '今天 ' + new Date((Date.now() / 1000 - sec) * 1000).toTimeString().slice(0, 5);
    if (sec < 86400 * 2) return '昨天';
    return Math.floor(sec / 86400) + ' 天前';
  }
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
  // unix 秒 → 10-19 08:32（本机时区）
  function fmtClock(ts) {
    if (ts == null || isNaN(ts)) return '—';
    const d = new Date(ts * 1000);
    const p2 = (n) => String(n).padStart(2, '0');
    return (d.getMonth() + 1) + '-' + p2(d.getDate()) + ' ' + p2(d.getHours()) + ':' + p2(d.getMinutes());
  }

  // ---------- 历史曲线 ----------
  let HIST_LEN = 120;
  // 绘图点数：历史照旧按秒存 120 个，但只画 80 段折线（240px 宽下每段 3px），
  // 否则每秒一个点挤在一起会显得毛刺很重。
  const PLOT_MAX = 80;
  const hist = { cpu: [], gpu: [], mem: [], diskR: [], diskW: [], netDown: [], netUp: [] };
  const COLORS = { cpu: '#0078d4', gpu: '#7a48c8', mem: '#0f9d58', disk: '#7c7c86', down: '#2f8fd6', up: '#e08a1e' };
  function pushHist(key, v) {
    hist[key].push(v);
    if (hist[key].length > HIST_LEN) hist[key].shift();
  }
  // 等间隔抽稀到 target 个点，首尾都保留（时间跨度不变，只是折线段变少）
  function decimate(arr, target) {
    const n = arr.length;
    if (n <= target) return arr;
    const out = [];
    for (let i = 0; i < target; i++) out.push(arr[Math.round((i * (n - 1)) / (target - 1))]);
    return out;
  }

  function sparkline(svg, data, color, max) {
    if (!svg || !data.length || max == null) return;
    const vb = svg.getAttribute('viewBox').split(' ');
    const w = +vb[2], h = +vb[3];
    const d = decimate(data, PLOT_MAX);
    // x 轴按固定槽位排，曲线从左边开始向右生长，满了才铺到右端
    const step = w / (Math.min(PLOT_MAX, HIST_LEN) - 1);
    const cap = max > 0 ? max : 1;
    const pts = d.map((v, i) =>
      (i * step).toFixed(1) + ',' + (h - (Math.min(v, cap) / cap) * (h - 3) - 1.5).toFixed(1)
    );
    if (pts.length < 2) { svg.innerHTML = ''; return; }
    const p = pts.join(' ');
    const xEnd = ((d.length - 1) * step).toFixed(1);
    svg.innerHTML =
      '<polygon points="0,' + h + ' ' + p + ' ' + xEnd + ',' + h + '" fill="' + color + '" fill-opacity="0.14"/>' +
      '<polyline points="' + p + '" fill="none" stroke="' + color + '" stroke-width="1.6" stroke-linejoin="round"/>';
  }

  // 环形图：中心显示百分比（电池电量用）
  const RING_R = 42;
  const RING_C = 2 * Math.PI * RING_R;
  function ringChart(svg, pct, color, label) {
    if (!svg) return;
    if (!svg.dataset.built) {
      svg.innerHTML =
        '<circle class="track" cx="50" cy="50" r="' + RING_R + '" fill="none" stroke-width="9"/>' +
        '<circle class="arc" cx="50" cy="50" r="' + RING_R + '" fill="none" stroke-width="9" stroke-linecap="round" ' +
        'transform="rotate(-90 50 50)" stroke-dasharray="' + RING_C.toFixed(1) + '"/>' +
        '<text class="pct" x="50" y="50" text-anchor="middle" dominant-baseline="central">—</text>';
      svg.dataset.built = '1';
    }
    const p = Math.max(0, Math.min(100, pct || 0));
    const arc = svg.querySelector('.arc');
    if (arc) {
      arc.setAttribute('stroke', color);
      arc.setAttribute('stroke-dashoffset', (RING_C * (1 - p / 100)).toFixed(1));
    }
    const txt = svg.querySelector('.pct');
    if (txt) txt.textContent = label != null ? label : p.toFixed(0) + '%';
  }

  // 电量颜色：充电蓝、正常绿、偏低琥珀、告急红
  function batColor(b, pct) {
    if (b && b.charging) return '#0078d4';
    if (pct <= 15) return '#d83b3b';
    if (pct <= 35) return '#d99a00';
    return '#0f9d58';
  }

  // 分段刻度条（仪表盘）：24 格，最后一格按小数部分点亮，读数不看数字也能估个大概
  const METER_N = 24;
  function meter(el, pct, color) {
    if (!el) return;
    if (!el.dataset.built) {
      let h = '';
      for (let i = 0; i < METER_N; i++) h += '<i></i>';
      el.innerHTML = h;
      el.dataset.built = '1';
    }
    el.style.setProperty('--c', color);
    const p = Math.max(0, Math.min(100, pct == null || isNaN(pct) ? 0 : pct));
    for (let i = 0; i < METER_N; i++) {
      const lo = (i * 100) / METER_N;
      const frac = Math.max(0, Math.min(1, (p - lo) / (100 / METER_N)));
      el.children[i].style.setProperty('--f', (frac * 100).toFixed(0) + '%');
    }
  }

  // 块状电池：12 格电量块，右侧一个小正极头
  const BAT_CELLS = 12;
  function batteryBlocks(el, pct, color, charging) {
    if (!el) return;
    if (!el.dataset.built) {
      let h = '';
      for (let i = 0; i < BAT_CELLS; i++) h += '<i></i>';
      el.innerHTML = h;
      el.dataset.built = '1';
    }
    el.style.setProperty('--c', color);
    el.classList.toggle('charging', !!charging);
    const p = Math.max(0, Math.min(100, pct == null || isNaN(pct) ? 0 : pct));
    for (let i = 0; i < BAT_CELLS; i++) {
      const lo = (i * 100) / BAT_CELLS;
      const frac = Math.max(0, Math.min(1, (p - lo) / (100 / BAT_CELLS)));
      el.children[i].style.setProperty('--f', (frac * 100).toFixed(0) + '%');
    }
  }

  // 双曲线：两条线画在同一坐标系里，各自按自身峰值缩放
  // （上行 200 KB/s 对下行 3 MB/s，共用一套刻度的话上行永远是条直线）。
  // 数值放在曲线外面，不压在线上。
  function dualChart(svg, a, b, cA, cB, floorA, floorB) {
    if (!svg) return;
    if (!a.length && !b.length) { svg.innerHTML = ''; return; }
    const vb = svg.getAttribute('viewBox').split(' ');
    const w = +vb[2], h = +vb[3];
    const da = decimate(a, PLOT_MAX), db = decimate(b, PLOT_MAX);
    // 固定槽位步长：曲线从左边起画，满了才到右端
    const step = w / (Math.min(PLOT_MAX, HIST_LEN) - 1);
    const peak = (arr, floor) => Math.max(floor, Math.max.apply(null, arr.length ? arr : [0]) * 1.08);
    const geom = (data, cap) => {
      if (data.length < 2) return null;
      const pts = data.map((v, i) => {
        const y = h - 1.5 - (Math.min(v, cap) / cap) * (h - 4);
        return (i * step).toFixed(1) + ',' + y.toFixed(1);
      }).join(' ');
      return { pts: pts, xEnd: ((data.length - 1) * step).toFixed(1) };
    };
    const ga = geom(da, peak(a, floorA));
    const gb = geom(db, peak(b, floorB));
    const area = (g, c) => (g
      ? '<polygon points="0,' + h + ' ' + g.pts + ' ' + g.xEnd + ',' + h + '" fill="' + c + '" fill-opacity="0.11"/>'
      : '');
    const line = (g, c, sw) => (g
      ? '<polyline points="' + g.pts + '" fill="none" stroke="' + c + '" stroke-width="' + sw + '" stroke-linejoin="round"/>'
      : '');
    // 先画面积再画线，后画的线压在上面（细线 b 压粗线 a 更容易分辨）
    svg.innerHTML = area(ga, cA) + area(gb, cB) + line(ga, cA, 1.8) + line(gb, cB, 1.4);
  }

  // 磁盘占用越高越红
  const usageColor = (p) => (p >= 90 ? '#d83b3b' : p >= 75 ? '#d99a00' : '#2f8fd6');

  // 状态词：[文字, 语气]（语气：'' | warn | bad）
  function setSt(id, pair) {
    const el = $(id);
    if (!el || !pair) return;
    el.textContent = pair[0];
    el.className = 'st' + (pair[1] ? ' ' + pair[1] : '');
  }
  const loadWord = (v) => ((v || 0) < 30 ? ['低负载', ''] : (v || 0) < 70 ? ['中等负载', ''] : ['高负载', 'warn']);
  const memWord = (v) => ((v || 0) < 60 ? ['占用较低', ''] : (v || 0) < 85 ? ['占用较高', 'warn'] : ['占用很高', 'bad']);
  const netWord = (bps) => (bps > 0 ? ['正在传输', ''] : ['暂无传输', '']);
  const diskWord = (v) => ((v || 0) < 5 ? ['空闲', ''] : (v || 0) < 55 ? ['正常', ''] : (v || 0) < 85 ? ['繁忙', 'warn'] : ['满载', 'bad']);
  // 温度没有有效传感器读数时保留缺失状态，不猜测
  const tempValue = (c) => (typeof c === 'number' && Number.isFinite(c) ? Math.round(c) + ' ℃' : '—');
  const tempText = (c) => tempValue(c) === '—' ? '温度 —' : tempValue(c);

  // 卡片右上角补充值（温度 / 网卡名），过热标红
  function setMeta(id, text, hot) {
    const el = $(id);
    if (!el) return;
    el.textContent = text;
    el.title = text;
    el.classList.toggle('hot', !!hot);
  }
  // 仪表盘大号百分比：数字大、单位小
  function bigPct(id, v) {
    const el = $(id);
    if (!el) return;
    if (v == null || isNaN(v)) { el.textContent = '—'; return; }
    el.innerHTML = Math.round(v) + '<span class="u">%</span>';
  }

  // 柱状图（读/写堆叠，null = 未运行）—— 对应设计稿的 24h / 30 天
  function barsChart(svg, slots, rC, wC, eC) {
    if (!svg) return;
    const vb = svg.getAttribute('viewBox').split(' ');
    const w = +vb[2], h = +vb[3];
    const step = w / slots.length;
    const bw = Math.max(2, step * 0.76);
    let max = 1;
    slots.forEach((d) => { if (d) max = Math.max(max, d.read + d.write); });
    svg.innerHTML = slots.map((d, i) => {
      const x = (i * step + (step - bw) / 2).toFixed(1);
      if (!d) {
        return '<rect x="' + x + '" y="' + (h - 4) + '" width="' + bw.toFixed(1) +
          '" height="4" rx="2" fill="currentColor" fill-opacity="0.18" stroke="currentColor" stroke-opacity="0.16" stroke-width="0.5"/>';
      }
      const h1 = (d.read / max) * (h - 6);
      const h2 = (d.write / max) * (h - 6);
      let out = '';
      if (h1 > 0.5) out += '<rect x="' + x + '" y="' + (h - h1).toFixed(1) + '" width="' + bw.toFixed(1) + '" height="' + h1.toFixed(1) + '" rx="2" fill="' + rC + '"/>';
      if (h2 > 0.5) out += '<rect x="' + x + '" y="' + (h - h1 - h2 - 1).toFixed(1) + '" width="' + bw.toFixed(1) + '" height="' + h2.toFixed(1) + '" rx="2" fill="' + wC + '"/>';
      return out;
    }).join('');
  }

  // 电池曲线：12 小时窗口，插电时段用绿底
  function batteryChart(svg, pts, now) {
    if (!svg) return;
    const WIN = 12 * 3600;
    const start = now - WIN;
    const list = (pts || []).filter((p) => p.ts >= start);
    const w = 600, h = 62;
    if (list.length < 2) {
      svg.innerHTML = '<text x="6" y="36" font-size="10" fill="currentColor" opacity="0.45">本次运行还没攒够数据点（每 30 秒一个）</text>';
      return;
    }
    const X = (t) => ((t - start) / WIN) * w;
    const Y = (p) => h - (Math.max(0, Math.min(100, p)) / 100) * (h - 8) - 4;
    let rects = '';
    let runStart = null;
    for (let i = 0; i < list.length; i++) {
      if (list[i].ac && runStart === null) runStart = list[i].ts;
      if ((!list[i].ac || i === list.length - 1) && runStart !== null) {
        const end = list[i].ts;
        rects += '<rect x="' + X(runStart).toFixed(1) + '" y="0" width="' + Math.max(1, X(end) - X(runStart)).toFixed(1) +
          '" height="' + h + '" fill="#0f9d58" fill-opacity="0.10"/>';
        runStart = null;
      }
    }
    const line = list.map((p, i) => (i ? 'L' : 'M') + X(p.ts).toFixed(1) + ',' + Y(p.pct).toFixed(1)).join(' ');
    const area = line + ' L' + X(list[list.length - 1].ts).toFixed(1) + ',' + h + ' L' + X(list[0].ts).toFixed(1) + ',' + h + ' Z';
    svg.innerHTML = rects +
      '<path d="' + area + '" fill="#0f9d58" fill-opacity="0.14"/>' +
      '<path d="' + line + '" fill="none" stroke="#0f9d58" stroke-width="1.6" stroke-linejoin="round"/>';
  }

  // ---------- 导航 / 主题 / 窗口 ----------
  // 当前页和最近一帧数据。渲染只针对可见页，切页时用最近一帧补画，
  // 免得每秒把九个页面（含 canvas 重绘）全部重做一遍。
  let curPage = 'overview';
  let lastSample = null;
  // 切页补画时不要再把同一帧推进历史，否则曲线会被重复点污染。
  let repainting = false;

  function goPage(p) {
    if (!document.querySelector('.page[data-p="' + p.replace(/[^a-z]/g, '') + '"]')) p = 'overview';
    document.querySelectorAll('#side .ni').forEach((x) => x.classList.toggle('on', x.dataset.p === p));
    document.querySelectorAll('.page').forEach((pg) => pg.classList.toggle('on', pg.dataset.p === p));
    document.querySelectorAll('#side .ni').forEach((x) => x.setAttribute('aria-current', x.dataset.p === p ? 'page' : 'false'));
    document.querySelector('.content').scrollTop = 0;
    curPage = p;
    if (lastSample) {
      repainting = true;
      try { render(lastSample); } finally { repainting = false; }
    }
  }
  function initNav() {
    document.querySelectorAll('#side .ni').forEach((ni) => {
      ni.onclick = () => goPage(ni.dataset.p);
      ni.tabIndex = 0;
      ni.setAttribute('role', 'button');
      ni.onkeydown = (e) => {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); ni.click(); }
      };
    });
    // IP 卡片：点一下复制
    ['net-ip-local', 'net-ip-egress', 'net-ip-proxy'].forEach((id) => {
      const el = $(id);
      if (el) el.onclick = () => {if (el.dataset.copyable === 'false') return;copyText(el, el.textContent.trim());};
    });
    $('net-refresh').onclick = async () => {
      const button=$('net-refresh');button.disabled=true;
      try {
        if (window.__TAURI__?.core?.invoke) await window.__TAURI__.core.invoke('refresh_network');
        button.textContent='已请求刷新';
      } catch {button.textContent='刷新失败';}
      finally {setTimeout(()=>{button.disabled=false;button.textContent='刷新出口 IP';},2000);}
    };
  }
  function initTheme() {
    $('btnTheme').onclick = () => window.WinMeterSettings.toggleTheme();
  }
  function initWindowControls() {
    const api = window.__TAURI__;
    const cur = () => {
      try {
        if (api && api.window && api.window.getCurrentWindow) return api.window.getCurrentWindow();
        if (api && api.window && api.window.getCurrent) return api.window.getCurrent();
      } catch (e) { /* ignore */ }
      return null;
    };
    $('btnMin').onclick = () => {
      if (api?.core?.invoke) api.core.invoke('minimize_main').catch(() => { const w = cur(); if (w) w.minimize(); });
      else { const w = cur(); if (w) w.minimize(); }
    };
    $('btnMax').onclick = () => { const w = cur(); if (w) w.toggleMaximize(); };
    $('btnClose').onclick = () => { const w = cur(); if (w) w.close(); };
  }
  // ---------- 资源页进程明细 ----------
  function sortedProcs(procs, key, desc) {
    const arr = (procs || []).slice();
    const val = (p) => {
      switch (key) {
        case 'name': return p.name.toLowerCase();
        case 'pid': return p.pid;
        case 'mem': return p.mem_bytes == null ? -1 : p.mem_bytes;
        case 'dr': return p.disk_read_bps == null ? -1 : p.disk_read_bps;
        case 'dw': return p.disk_write_bps == null ? -1 : p.disk_write_bps;
        case 'gpu': return p.gpu_percent == null ? -1 : p.gpu_percent;
        default: return p.cpu == null ? -1 : p.cpu;
      }
    };
    arr.sort((a, b) => {
      const va = val(a), vb = val(b);
      if (va < vb) return desc ? 1 : -1;
      if (va > vb) return desc ? -1 : 1;
      return 0;
    });
    return arr;
  }

  function barCell(pct, color, label, text) {
    const bar = '<span class="pbar" title="' + esc(label || ('使用率 ' + (pct || 0).toFixed(1) + '%')) + '"><i style="width:' +
      Math.max(0, Math.min(100, pct || 0)).toFixed(2) + '%;background:' + color + '"></i></span>';
    if (text == null) return '<td class="r">' + bar + '</td>';
    return '<td class="r"><span class="volcell">' + bar + '<span class="pv">' + esc(text) + '</span></span></td>';
  }

  // 占比条比的量：CPU% / 内存 / 磁盘读 / GPU
  function barVal(p, kind) {
    let value;
    switch (kind) {
      case 'mem': value = p.mem_bytes; break;
      case 'dr': value = p.disk_read_bps; break;
      case 'dw': value = p.disk_write_bps; break;
      case 'gpu': value = p.gpu_percent; break;
      default: value = p.cpu;
    }
    return Number.isFinite(value) && value >= 0 ? value : null;
  }

  function procRow(p, o, widthPct) {
    let html = '<tr><td class="n">' + esc(p.name) + '</td>';
    if (o.pid) html += '<td class="r">' + p.pid + '</td>';
    if (o.cpu !== false) html += '<td class="r n">' + (p.cpu == null ? '—' : p.cpu.toFixed(1) + '%') + '</td>';
    if (o.mem !== false) html += '<td class="r">' + (p.mem_bytes == null ? '—' : fmtBytes(p.mem_bytes)) + '</td>';
    if (o.disk) {
      html += '<td class="r">' + (p.disk_read_bps != null ? fmtRate(p.disk_read_bps) : '—') + '</td>';
      html += '<td class="r">' + (p.disk_write_bps != null ? fmtRate(p.disk_write_bps) : '—') + '</td>';
    }
    if (o.ops) {
      html += '<td class="r">' + (p.disk_ops != null ? Math.round(p.disk_ops).toLocaleString('zh-CN') : '—') + '</td>';
    }
    if (o.gpu) html += '<td class="r">' + (p.gpu_percent == null
      ? '<span style="color:var(--faint)">·</span>'
      : p.gpu_percent.toFixed(1) + '%') + '</td>';
    if (o.bar) {
      const value = barVal(p, o.bar);
      const pct = Math.max(0, Math.min(100, widthPct || 0));
      const actual = o.bar === 'mem'
        ? '内存占用 ' + fmtBytes(value) + '，占物理内存 ' + pct.toFixed(1) + '%'
        : 'CPU 使用率 ' + (value ?? 0).toFixed(1) + '%';
      html += value === null ? '<td class="r">—</td>'
        : barCell(widthPct, o.barColor || '#0078d4', actual, o.bar === 'mem' ? pct.toFixed(1) + '%' : null);
    }
    return html + '</tr>';
  }

  // Compare only listed processes. Numeric CPU percentages still use whole-
  // machine capacity; missing readings must not be drawn as measured zero.
  // Memory bars use physical RAM as 100%, never the largest row in the table.
  function renderRows(list, opts) {
    return list.map((p) => {
      let widthPct = 0;
      if (opts.bar) {
        const value = barVal(p, opts.bar);
        if (value !== null) {
          const scale = opts.bar === 'mem' ? (opts.total || 0) : 100;
          widthPct = scale > 0 ? (value / scale) * 100 : 0;
        }
      }
      return procRow(p, opts, widthPct);
    }).join('');
  }

  function renderTopProcTable(bodyId, procs, sortKey, limit, opts) {
    const body = $(bodyId);
    if (!body) return;
    const top = sortedProcs(procs, sortKey, true).slice(0, limit);
    body.innerHTML = renderRows(top, opts);
  }

  // ---------- 各屏渲染 ----------
  function renderCores(cores, count) {
    const cg = $('cores');
    if (!cg) return;
    if (!cores || !cores.length) { cg.innerHTML = ''; set('cores-legend', '等待采样…'); return; }
    const circumference = 2 * Math.PI * 26;
    cg.innerHTML = cores.map((v, i) => {
      const pct = Math.max(0, Math.min(100, Number.isFinite(v) ? v : 0));
      const color = pct >= 90 ? '#dc4545' : pct >= 70 ? '#d18a18' : COLORS.cpu;
      return '<div class="core-item"><svg viewBox="0 0 64 64" role="img" aria-label="逻辑处理器 ' + i + '：' + pct.toFixed(0) + '%">' +
        '<circle class="core-track" cx="32" cy="32" r="26"/>' +
        '<circle class="core-value" cx="32" cy="32" r="26" stroke="' + color + '" stroke-dasharray="' + circumference.toFixed(2) + '" stroke-dashoffset="' + (circumference * (1 - pct / 100)).toFixed(2) + '"/>' +
        '<text x="32" y="33" dominant-baseline="middle" text-anchor="middle">' + pct.toFixed(0) + '<tspan class="core-unit">%</tspan></text></svg>' +
        '<span>CPU ' + i + '</span></div>';
    }
    ).join('');
    const avg = cores.reduce((a, b) => a + b, 0) / cores.length;
    const max = Math.max.apply(null, cores);
    const legendEl = $('cores-legend');
    if (legendEl) {
      legendEl.innerHTML = '<span>共 ' + (count || cores.length) + ' 个逻辑处理器</span>' +
        '<span>平均 <b style="color:var(--text)">' + avg.toFixed(0) + '%</b></span>' +
        '<span>最高 <b style="color:var(--text)">' + max.toFixed(0) + '%</b></span>';
    }
  }

  function renderMemBar(s) {
    const bar = $('membar');
    if (!bar) return;
    const total = s.mem_total_gb || 1;
    // 可用已包含备用；缓存=备用+已修改，不能再作为独立分段累计。
    const standby = Math.min(s.mem_standby_gb || 0, s.mem_available_gb || 0);
    const free = Math.max(0, (s.mem_available_gb || 0) - standby);
    const segs = [
      [s.mem_in_use_gb / total * 100, 'var(--mem-used)', '已使用', s.mem_in_use_gb],
      [s.mem_modified_gb / total * 100, 'var(--mem-modified)', '待写回内存', s.mem_modified_gb],
      [standby / total * 100, 'var(--mem-standby)', '备用缓存', standby],
      [free / total * 100, 'var(--mem-free)', '空闲', free],
    ];
    bar.innerHTML = segs.map((seg) =>
      '<span title="' + seg[2] + ' ' + fmtGB(seg[3]) + '" style="width:' + Math.max(0, seg[0]).toFixed(2) + '%;background:' + seg[1] + '"></span>'
    ).join('');
    const legend = $('mem-legend');
    if (legend) {
      legend.innerHTML =
        '<span><s style="background:var(--mem-used)"></s>已使用 ' + fmtGB(s.mem_in_use_gb) + '</span>' +
        '<span title="内容已修改，写入磁盘后才能回收"><s style="background:var(--mem-modified)"></s>待写回内存 ' + fmtGB(s.mem_modified_gb) + '</span>' +
        '<span title="可在程序需要时回收的缓存"><s style="background:var(--mem-standby)"></s>备用缓存 ' + fmtGB(standby) + '</span>' +
        '<span><s style="background:var(--mem-free)"></s>空闲 ' + fmtGB(free) + '</span>' +
        '<span>可用 ' + fmtGB(s.mem_available_gb) + ' · 缓存 ' + fmtGB(s.mem_cached_gb) + '（含备用与已修改）</span>';
    }
  }

  // ---------- 网络页 ----------

  // 速率拆成「数字 + 单位」，大号读数里单位用小字
  function splitRate(bps) {
    const t = fmtRate(bps);
    const i = t.lastIndexOf(' ');
    return i < 0 ? [t, ''] : [t.slice(0, i), t.slice(i + 1)];
  }
  function setBigRate(id, bps) {
    const el = $(id);
    if (!el) return;
    const [n, u] = splitRate(bps);
    el.innerHTML = n + '<span class="u"> ' + u + '</span>';
  }
  // 信号质量 → 文字
  function wifiWord(q) {
    if (q == null) return ['—', ''];
    return q >= 75 ? ['信号强', 'ok'] : q >= 50 ? ['信号良', ''] : q >= 30 ? ['信号弱', 'warn'] : ['信号差', 'bad'];
  }
  function copyText(el, text) {
    if (!text || text === '—' || text === '获取中…' || text === '未连接') return;
    const done = () => {
      el.classList.add('copied');
      setTimeout(() => el.classList.remove('copied'), 700);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, () => { if (fallbackCopy(text)) done(); });
    } else if (fallbackCopy(text)) {
      done();
    }
  }
  function fallbackCopy(text) {
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.cssText = 'position:fixed;left:-9999px';
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand('copy');
      document.body.removeChild(ta);
      return ok;
    } catch (e) {
      return false;
    }
  }

  function renderNet(s) {
    const n = s.net || {};
    const ad = s.adapters || [];
    const peakDn = Math.max.apply(null, hist.netDown.length ? hist.netDown : [0]);
    const peakUp = Math.max.apply(null, hist.netUp.length ? hist.netUp : [0]);

    // 标题栏：网卡 · 类型 · 延迟
    const bits = [];
    if (n.if_name) bits.push(n.if_name);
    if (n.kind) bits.push(n.kind);
    if (n.rtt_ms != null) bits.push(Math.round(n.rtt_ms) + ' ms');
    set('net-bs', bits.join(' · ') || '—');

    // ① 两个实时框
    setBigRate('net-dn-v', s.net_down_bps);
    setBigRate('net-up-v', s.net_up_bps);
    set('net-dn-peak', '峰值 ' + fmtRate(peakDn));
    set('net-up-peak', '峰值 ' + fmtRate(peakUp));
    set('net-dn-s', '本次运行累计 ' + fmtBytes(s.net_session_down_bytes || 0));
    set('net-up-s', '本次运行累计 ' + fmtBytes(s.net_session_up_bytes || 0));
    setSt('net-dn-st', netWord(s.net_down_bps || 0));
    setSt('net-up-st', netWord(s.net_up_bps || 0));

    // ② 曲线
    set('net-up-v2', '↑ ' + fmtRate(s.net_up_bps));
    set('net-down-v2', '↓ ' + fmtRate(s.net_down_bps));
    set('net-leg-up', fmtRate(peakUp));
    set('net-leg-dn', fmtRate(peakDn));
    dualChart($('ch-net'), hist.netDown, hist.netUp, COLORS.down, COLORS.up, 64 * 1024, 16 * 1024);

    // ③ 适配器
    set('net-adapter-sub', ad.length + ' 个接口 · 总量不重复累计 TUN');
    renderAdapters(ad);

    // ④ 物理链路内网 IP，不用 TUN 假地址冒充
    const hasLocal = !!n.local_ip;
    const ipEl = $('net-ip-local');
    if (ipEl) {
      ipEl.textContent = hasLocal ? n.local_ip : '未连接';
      ipEl.className = 'dip' + (hasLocal ? '' : ' dim');
    }
    setSt('net-ip-who', hasLocal ? ['已连接', 'ok'] : ['离线', 'warn']);
    set('net-ip-kind', n.kind || '—');
    const localBits = [];
    if (n.mac) localBits.push(n.mac);
    if ((n.gateways || []).length) localBits.push('网关 ' + n.gateways[0]);
    set('net-ip-local-s', localBits.join(' · ') || '—');

    // ⑤ 出口 IP
    const hasEg = !!n.egress_ip;
    const egEl = $('net-ip-egress');
    if (egEl) {
      egEl.textContent = hasEg ? n.egress_ip : n.egress_error ? '暂不可用' : '获取中…';
      egEl.className = 'dip' + (hasEg ? '' : ' dim');
      egEl.dataset.copyable = String(hasEg);
    }
    setSt('net-eg-st', hasEg
      ? ['已获取', 'ok']
      : n.egress_error ? ['查询失败', 'warn'] : ['探测中', '']);
    set('net-eg-who', n.egress_at && n.egress_at > 0
      ? fmtAgo(Math.max(0, Math.floor(Date.now() / 1000) - n.egress_at)) + '更新'
      : '每 2 分钟刷新');
    const egBits = [];
    if (n.egress_region) egBits.push(n.egress_region);
    if (n.egress_isp) egBits.push(n.egress_isp);
    set('net-eg-s', egBits.join(' · ') || n.egress_error || 'IPIP 国内服务 · 绕过 HTTP 代理');

    const interfaces = n.interfaces || [];
    const tunnels = interfaces.filter(i => i.up && i.is_tun);
    const routes = n.routes || [];
    const routedTun = routes.some(r => r.is_tun);
    setSt('net-tun-st', routedTun ? ['路由经过', 'ok'] : tunnels.length ? ['接口已连接', ''] : ['未检测到', '']);
    set('net-tun-name', tunnels.map(i => i.name).join(' · ') || '未发现活动隧道');
    set('net-tun-s', tunnels.length ? tunnels.map(i => (i.ipv4 || i.ipv6 || '无地址') + ' · MTU ' + i.mtu).join(' / ')
      : '根据网卡类型与驱动名称识别 TUN / VPN');
    if (hasEg && tunnels.length) set('net-eg-who', '可能经 TUN' + (n.egress_at ? ' · ' + fmtAgo(Math.max(0, Date.now() / 1000 - n.egress_at)) + '更新' : ''));

    const manualProxy = n.proxy_enabled && !!n.proxy_server && !n.proxy_server.includes('=');
    const proxyConfigured = manualProxy || tunnels.length > 0;
    const hasProxy = proxyConfigured && !!n.proxy_ip;
    const proxyEl = $('net-ip-proxy');
    if (proxyEl) {
      proxyEl.textContent = hasProxy ? n.proxy_ip : proxyConfigured ? n.proxy_error ? '暂不可用' : '获取中…' : '—';
      proxyEl.className = 'dip' + (hasProxy ? '' : ' dim');
      proxyEl.dataset.copyable = String(hasProxy);
    }
    setSt('net-proxy-st', hasProxy ? [n.proxy_path === 'tun' ? 'TUN 实测' : '代理实测', 'ok'] : proxyConfigured ? n.proxy_error ? ['查询失败', 'warn'] : ['探测中', ''] : n.pac_url ? ['PAC 模式', ''] : ['未配置', '']);
    set('net-proxy-who', hasProxy && n.proxy_at ? fmtAgo(Math.max(0, Date.now() / 1000 - n.proxy_at)) + '更新' : '独立探测');
    set('net-proxy-s', proxyConfigured ? (n.proxy_error || (manualProxy ? n.proxy_server : '系统路由 · TUN / VPN') + ' · ipify 实测')
      : n.pac_url ? '检测到 PAC，暂不解析代理脚本' : '未发现系统代理或活动 TUN');

    const routeBody = $('net-routes');
    if (routeBody) routeBody.innerHTML = ['223.5.5.5', '1.1.1.1'].map(target => {
      const r = routes.find(r => r.target === target);
      return '<div class="network-route"><small>' + (target === '223.5.5.5' ? '国内目标' : '外部目标') + ' · ' + target + '</small><strong>'
        + esc(r ? r.if_name + (r.is_tun ? ' · TUN / VPN' : '') : '无可用路由') + '</strong><small>'
        + esc(r ? '源地址 ' + (r.source || '—') + ' → 下一跳 ' + (!r.next_hop || r.next_hop === '0.0.0.0' ? '目标直达' : r.next_hop) + ' · 匹配前缀 /' + r.prefix_len : '等待网络连接') + '</small></div>';
    }).join('');
    const interfaceBody = $('net-interfaces');
    if (interfaceBody) interfaceBody.innerHTML = interfaces.filter(i => i.up).map(i => '<div class="network-interface"><div><strong>'
      + esc(i.name) + '</strong> <span class="sub">' + esc(i.is_tun ? 'TUN / VPN' : i.kind) + '</span><small>' + esc(i.description)
      + '</small></div><div class="network-address">' + esc([i.ipv4, i.ipv6].filter(Boolean).join(' · ') || '无地址') + '<small>'
      + esc('MTU ' + i.mtu + ' · DNS ' + (i.dns || []).join(' / ')) + '</small></div></div>').join('') || '<div class="note">没有已连接的网卡</div>';

    // ⑥ 链路详情（四列两对）
    // 网关 / MAC 已在上面的 IP 卡副行，这里不重复
    const rows = [
      ['网卡', (n.if_name || '—') + (n.kind ? ' · ' + n.kind : ''), '网卡连接速率',
        n.link_mbps ? Math.round(n.link_mbps) + ' Mbps' : '—'],
      ['Wi-Fi', n.wifi_ssid || '—', '信号',
        n.wifi_signal != null ? n.wifi_signal + '% · ' + wifiWord(n.wifi_signal)[0] : '—'],
      ['ICMP 延迟', n.rtt_ms != null ? Math.round(n.rtt_ms) + ' ms → ' + (n.rtt_target || '—') : '无应答（可能禁用 ICMP）', '系统代理',
        n.proxy_enabled ? (n.proxy_server || '已开启') : n.pac_url ? 'PAC · ' + n.pac_url : '未开启'],
      ['DNS', (n.dns || []).join(' · ') || '—', 'IPv6', n.ipv6 || '—'],
    ];
    const body = $('net-detail-body');
    if (body) {
      body.innerHTML = rows.map((r) =>
        '<tr><td class="k">' + esc(r[0]) + '</td><td class="v">' + esc(r[1]) +
        '</td><td class="k">' + esc(r[2]) + '</td><td class="v">' + esc(r[3]) + '</td></tr>'
      ).join('');
    }

    // 信号差 → 顺带提示（用 title 挂在 Wi-Fi 那行）
  }

  function renderAdapters(adapters, bodyId) {
    const body = $(bodyId || 'net-adapter-body');
    if (!body) return;
    if (!adapters || !adapters.length) {
      body.innerHTML = '<tr><td class="n" colspan="4" style="color:var(--faint)">未检测到活动适配器</td></tr>';
      return;
    }
    const maxDown = adapters.reduce((m, a) => Math.max(m, a.down_bps || 0), 1);
    body.innerHTML = adapters.map((a) => {
      const pct = Math.min(100, (a.down_bps || 0) / maxDown * 100);
      return '<tr><td class="n">' + esc(a.name) + '</td>' +
        '<td class="r n">' + fmtRate(a.down_bps) + '</td>' +
        '<td class="r">' + fmtRate(a.up_bps) + '</td>' +
        barCell(pct, 'var(--down)', '当前下载速度为本表最高值的 ' + pct.toFixed(1) + '%') + '</tr>';
    }).join('');
  }

  // 概览：磁盘卷列表（每卷一条占用条 + 百分比）
  function renderOverviewVolumes(s) {
    const body = $('ov-vol-body');
    if (!body) return;
    const vols = (s.volumes || []).filter((v) => v && v.total_bytes > 0);
    let totalAll = 0;
    let usedAll = 0;
    vols.forEach((v) => {
      totalAll += v.total_bytes;
      usedAll += Math.max(0, v.total_bytes - v.free_bytes);
    });
    set('ov-disk-total', totalAll
      ? '合计 ' + fmtPair(usedAll, totalAll) + ' · 已用 ' + (usedAll / totalAll * 100).toFixed(0) + '%'
      : '');
    if (!vols.length) {
      body.innerHTML = '<tr><td class="n" colspan="5" style="color:var(--faint)">未枚举到卷</td></tr>';
      return;
    }
    body.innerHTML = vols.map((v) => {
      const used = Math.max(0, v.total_bytes - v.free_bytes);
      const pct = (used / v.total_bytes) * 100;
      const color = usageColor(pct);
      return '<tr><td class="n">' + esc(v.mount) +
        (v.label ? ' <span class="faint">' + esc(v.label) + '</span>' : '') + '</td>' +
        '<td class="r n">' + fmtCap(used) + '</td>' +
        '<td class="r">' + fmtCap(v.free_bytes) + '</td>' +
        '<td class="r">' + fmtCap(v.total_bytes) + '</td>' +
        '<td><div class="volcell"><div class="pbar"><i style="width:' + pct.toFixed(1) + '%;background:' + color + '"></i></div>' +
        '<span class="pv" style="color:' + color + '">' + pct.toFixed(0) + '%</span></div></td></tr>';
    }).join('');
  }

  // 显卡页：每张卡一行（集显 / 独显分开算）
  function renderGpus(gpus) {
    const list = gpus || [];
    const body = $('gpu-body');
    const discrete = list.filter((g) => g.discrete).length;
    set('gpu-count', list.length || '—');
    set('gpu-count-s', list.length ? discrete + ' 独显 · ' + (list.length - discrete) + ' 集显' : '等待系统返回显卡信息');
    if (!body) return;
    if (!list.length) {
      body.innerHTML = '<tr><td class="n" colspan="6" style="color:var(--faint)">未枚举到显卡</td></tr>';
      return;
    }
    body.innerHTML = list.map((g) => {
      const pct = Math.max(0, Math.min(100, g.usage || 0));
      const mem = (g.mem_total_gb || 0) > 0
        ? ((g.mem_used_gb || 0) > g.mem_total_gb
          ? (g.mem_used_gb || 0).toFixed(1) + ' GB（含共享）'
          : (g.mem_used_gb || 0).toFixed(1) + ' / ' + g.mem_total_gb.toFixed(1) + ' GB')
        : '—';
      return '<tr><td class="n">' + esc(g.name) + '</td>' +
        '<td>' + (g.discrete ? '<span class="badge ok">独显</span>' : '<span class="badge">集显</span>') + '</td>' +
        '<td class="r n">' + tempValue(g.temp_c) + '</td>' +
        '<td class="r n">' + pct.toFixed(0) + '%</td>' +
        '<td class="r">' + mem + '</td>' +
        barCell(pct, '#7a48c8') + '</tr>';
    }).join('');
  }

  function renderVolumes(volumes) {
    const body = $('disk-vol-body');
    if (!body) return;
    if (!volumes || !volumes.length) {
      body.innerHTML = '<tr><td class="n" colspan="5" style="color:var(--faint)">未枚举到卷</td></tr>';
      return;
    }
    body.innerHTML = volumes.map((v) => {
      const usedPct = v.total_bytes > 0 ? ((v.total_bytes - v.free_bytes) / v.total_bytes * 100) : 0;
      return '<tr><td class="n">' + esc(v.mount) +
        (v.label ? ' <span class="faint">' + esc(v.label) + '</span>' : '') + '</td>' +
        '<td class="r n">' + fmtBytes(v.free_bytes) + '</td>' +
        '<td class="r">' + fmtBytes(v.total_bytes) + '</td>' +
        '<td class="r">' + usedPct.toFixed(0) + '%</td>' +
        '<td><span class="badge' + (usedPct < 75 ? ' ok' : '') + '">' + (usedPct >= 90 ? '空间不足' : usedPct >= 75 ? '空间偏低' : '空间充足') + '</span></td></tr>';
    }).join('');
  }

  // 本地时间桶 key（unix 秒）
  const hourKeyOf = (ts) => {
    const d = new Date(ts * 1000); d.setMinutes(0, 0, 0); return Math.floor(d.getTime() / 1000);
  };
  const dayKeyOf = (ts) => {
    const d = new Date(ts * 1000); d.setHours(0, 0, 0, 0); return Math.floor(d.getTime() / 1000);
  };

  function renderDiskLedger(s, now) {
    set('du-today-r', fmtBytes(s.disk_today_read));
    set('du-today-w', fmtBytes(s.disk_today_write));
    const curH = hourKeyOf(now);
    const mapH = new Map((s.disk_hours || []).map((b) => [b.ts, b]));
    const slots = [];
    for (let i = 23; i >= 0; i--) slots.push(mapH.get(curH - i * 3600) || null);
    barsChart($('du24'), slots, '#2f8fd6', '#7c7c86', null);

    const curD = dayKeyOf(now);
    const mapD = new Map((s.disk_days || []).map((b) => [b.ts, b]));
    const dslots = [];
    for (let i = 29; i >= 0; i--) dslots.push(mapD.get(curD - i * 86400) || null);
    barsChart($('du30'), dslots, '#2f8fd6', '#7c7c86', null);
  }

  function renderBattery(s, now) {
    const b = s.battery || {};
    set('bat-percent', b.percent != null ? b.percent.toFixed(0) + '%' : '—');
    let health = '—', healthS = '未提供';
    if (b.design_capacity_mwh && b.full_capacity_mwh) {
      health = (b.full_capacity_mwh / b.design_capacity_mwh * 100).toFixed(0) + '%';
      healthS = '充满 ' + (b.full_capacity_mwh / 1000).toFixed(1) + ' / 设计 ' + (b.design_capacity_mwh / 1000).toFixed(1) + ' Wh';
    }
    set('bat-health', health);
    set('bat-health-s', healthS);
    $('bat-health').closest('.col').hidden = health === '—';
    set('bat-plan', s.power_plan || '—');
    set('bat-bm', b.percent != null ? b.percent.toFixed(0) + '%' : '—');
    let status;
    if (b.present === false) status = '未检测到电池';
    else if (b.charging) status = '充电中';
    else if (b.ac_line) status = '已接通电源';
    else status = '放电中';
    set('bat-bs', status + (b.remaining_sec != null
      ? (b.charging ? ' · 充满还需 ' + fmtHM(b.remaining_sec) : ' · 剩余 ' + fmtHM(b.remaining_sec))
      : ''));

    batteryChart($('ch-bat'), s.battery_history, now);

    const body = $('bat-detail-body');
    if (body) {
      const rows = [
        ['名称', b.name || null, '系统报告的电池型号'],
        ['电池类型', b.chemistry || null, '例如锂离子电池'],
        ['设计容量', b.design_capacity_mwh > 0 ? b.design_capacity_mwh + ' mWh' : null, '出厂标称容量，1000 mWh = 1 Wh'],
        ['当前充满容量', b.full_capacity_mwh > 0 ? b.full_capacity_mwh + ' mWh' : null, '健康度 = 当前充满容量 ÷ 设计容量'],
        ['充电循环次数', b.cycle_count != null ? b.cycle_count + ' 次' : null, '电池控制器记录的累计循环'],
      ].filter(r => r[1] != null);
      body.closest('.card').hidden = rows.length === 0;
      body.innerHTML = rows.map((r) =>
        '<tr><td class="n">' + r[0] + '</td><td>' + esc(r[1]) + '</td><td class="r">' + r[2] + '</td></tr>'
      ).join('');
    }
  }

  function renderBluetooth(bt) {
    if (!bt) return;
    set('bt-bm', bt.connected);
    set('bt-bs', '台已连接 · 已保存 ' + bt.remembered + ' 台');
    set('bt-connected', bt.connected);
    set('bt-paired', bt.remembered);
    set('bt-adapter', bt.adapter || '未提供');
    if (bt.adapter) $('bt-adapter').title = bt.adapter;
    set('bt-radio', bt.radio ? '蓝牙控制器 ' + bt.radio : '—');
    set('bt-unassoc', (bt.devices || []).filter(d => d.remembered && !d.connected).length);
    const body = $('bt-device-body');
    if (!body) return;
    const devs = (bt.devices || []).slice().sort((a, b) => (b.connected ? 1 : 0) - (a.connected ? 1 : 0));
    if (!devs.length) {
      body.innerHTML = '<tr><td class="n" colspan="4" style="color:var(--faint)">未发现蓝牙设备</td></tr>';
      return;
    }
    body.innerHTML = devs.map((d) => {
      const badge = d.connected ? '<span class="badge ok">已连接</span>' : '<span class="badge">' + (d.remembered ? '已保存 · 未连接' : '未连接') + '</span>';
      return '<tr><td class="n">' + esc(d.name) + '</td>' +
        '<td>' + esc(d.kind || '其他') + '</td>' +
        '<td class="r">' + (d.connected ? '正在使用' : fmtAgo(d.last_used_sec)) + '</td>' +
        '<td class="r">' + badge + '</td></tr>';
    }).join('');
  }

  // ---------- 本机信息 ----------
  let infoSig = '';
  function kvRows(bodyId, rows) {
    const el = $(bodyId);
    if (!el) return;
    el.innerHTML = rows.filter(([, v]) => v != null && v !== '').map(([k, v]) =>
      '<tr><td class="n" style="width:132px">' + esc(k) + '</td><td class="ell">' +
      (v == null || v === '' ? '—' : esc(v)) + '</td></tr>'
    ).join('');
  }
  function renderInfo(s) {
    const sys = s.sys || {};
    // 本机信息几乎是静态的，签名不变就不重绘，免得每秒闪烁
    const sig = JSON.stringify(sys) + '|' + s.core_count + '|' + s.gpu_name + '|' + (s.adapters || []).map((a) => a.name).join(',');
    set('info-uptime', fmtDuration(s.uptime_sec));
    set('info-boot', sys.boot_ts ? '开机 ' + fmtClock(sys.boot_ts) : '—');
    set('info-cores', (sys.cpu_physical_cores || s.core_count || 0) + ' 核');
    set('info-threads', (sys.cpu_logical_cores || s.core_count || 0) + ' 线程');
    set('info-mem', fmtGB(s.mem_total_gb));
    set('info-mem-free', '可用 ' + fmtGB(s.mem_available_gb));
    set('info-procs', s.proc_count);

    // 传感器：温度每秒都在变，不能放进下面的签名缓存里
    set('sensor-src', s.sensor_src === 'integrated' ? 'WinMeter 内置硬件采集' : s.sensor_src === 'builtin' ? '系统接口 / 显卡驱动' : '等待传感器数据');
    const tp = (v) => (v == null ? null : Math.round(v) + ' ℃');
    kvRows('info-sensor-body', [
      ['CPU', tp(s.cpu_temp_c)],
      ['显卡', tp(s.gpu_temp_c)],
      ['主板', tp(s.mb_temp_c)],
      ['磁盘温度', tp(s.disk_temp_c)],
      ['风扇转速', s.fan_rpm != null ? Math.round(s.fan_rpm) + ' 转/分钟' : null],
    ]);

    if (sig === infoSig) return;
    infoSig = sig;

    const adapters = s.adapters || [];
    const vols = (s.volumes || []).map((v) => v.mount + (v.label ? ' ' + v.label : '')).join('、');
    kvRows('info-sys-body', [
      ['主机名', sys.hostname],
      ['当前用户', sys.user],
      ['操作系统', sys.os_name || 'Windows'],
      ['版本', sys.os_version],
      ['内部版本', sys.build],
      ['体系结构', sys.arch],
      ['采样进程', s.proc_count + ' 个'],
    ]);
    kvRows('info-hw-body', [
      ['处理器', s.cpu_name],
      ['基础频率', s.cpu_base_mhz ? (s.cpu_base_mhz / 1000).toFixed(2) + ' GHz' : null],
      ['核心 / 线程', (sys.cpu_physical_cores || s.core_count) + ' 核 / ' + (sys.cpu_logical_cores || s.core_count) + ' 线程'],
      ['内存', fmtGB(s.mem_total_gb)],
      // 固件给的装机容量和 Windows 可用容量可能差几百 MB（核显预留等），
      // 两行并排显示，免得看着像算错了。
      ['内存规格', window.WinMeterMem.specText(s) === '—' ? null : window.WinMeterMem.specText(s)],
      ['显卡', s.gpu_name],
      ['主板', sys.board],
      ['BIOS', sys.bios],
      ['网卡', adapters.length ? adapters.map((a) => a.name).join('、') : null],
      ['卷', vols || null],
    ]);
    set('info-bs', [sys.hostname, sys.os_name].filter(Boolean).join(' · ') || '—');
  }

  // ---------- 主渲染 ----------
  function render(s) {
    lastSample = s;
    // 采样本身是好的，只是没画；这一行必须在 hidden 判断之前，
    // 否则窗口收起时会被误判成「采样中断」。
    lastSampleAt = Date.now();
    // 窗口收进托盘时不再画：看不见的重绘只是白烧 CPU。切回来下一帧会补上。
    if (document.hidden) return;
    window.WinMeterSettings.renderSample(s, curPage === 'settings');
    set('sample-status', demoMode ? '演示数据' : '实时采集中');
    $('sample-status').dataset.state = demoMode ? 'demo' : 'live';
    const now = Math.floor(Date.now() / 1000);
    // 只有可见页需要重画；其余页在切过去时用最近一帧补。
    const on = (...names) => names.indexOf(curPage) >= 0;

    // 侧栏实时小值
    set('side-cpu', fmtPct(s.cpu_usage));
    set('side-gpu', fmtPct(s.gpu_usage));
    set('side-mem', fmtPct(s.mem_load));
    set('side-net', fmtRateTiny(s.net_down_bps));
    set('side-disk', fmtPct(s.disk_active));
    set('side-battery', s.battery && s.battery.percent != null ? s.battery.percent.toFixed(0) + '%' : '—');
    set('side-bt', s.bluetooth ? s.bluetooth.connected + ' 台' : '—');

    // 历史
    if (!repainting) {
      pushHist('cpu', s.cpu_usage || 0);
      pushHist('gpu', s.gpu_usage || 0);
      pushHist('mem', s.mem_load || 0);
      pushHist('diskR', s.disk_read_bps || 0);
      pushHist('diskW', s.disk_write_bps || 0);
      pushHist('netDown', s.net_down_bps || 0);
      pushHist('netUp', s.net_up_bps || 0);
    }

    // 显卡名和显存概览页、显卡页都要用，先算好。
    const gpuName = s.gpu_name || '未枚举到显卡';
    const gpuMem = (s.gpu_mem_total_gb || 0) > 0
      ? '显存 ' + (s.gpu_mem_used_gb || 0).toFixed(1) + ' / ' + s.gpu_mem_total_gb.toFixed(1) + ' GB'
      : '';

    if (on('overview')) {
    set('ov-uptime', fmtDuration(s.uptime_sec));
    bigPct('ov-cpu-v', s.cpu_usage);
    set('ov-cpu-s', (s.core_count || 0) + ' 逻辑处理器 · ' + (s.cpu_base_mhz ? (s.cpu_base_mhz / 1000).toFixed(2) + ' GHz' : '—'));
    setMeta('ov-cpu-t', tempText(s.cpu_temp_c), s.cpu_temp_c >= 80);
    $('ov-cpu-t').title = s.cpu_temp_c == null
      ? 'CPU 温度需要启用 WinMeter 内置管理员采集'
      : 'CPU 封装温度' + (s.sensor_src === 'lhm' ? '（LibreHardwareMonitor）' : '');
    meter($('ov-cpu-m'), s.cpu_usage, COLORS.cpu);

    bigPct('ov-gpu-v', s.gpu_usage);
    const gpuSEl = $('ov-gpu-s');
    if (gpuSEl) {
      gpuSEl.textContent = gpuName;
      gpuSEl.title = gpuName + (gpuMem ? ' · ' + gpuMem : '');
    }
    setMeta('ov-gpu-t', tempText(s.gpu_temp_c), s.gpu_temp_c >= 85);
    meter($('ov-gpu-m'), s.gpu_usage, COLORS.gpu);

    bigPct('ov-mem-v', s.mem_load);
    set('ov-mem-s', fmtGB(s.mem_used_gb) + ' / ' + fmtGB(s.mem_total_gb));
    setMeta('ov-mem-t', '可用 ' + fmtGB(s.mem_available_gb));
    meter($('ov-mem-m'), s.mem_load, COLORS.mem);

    // 状态词（参考 HUD 的「低负载 / 压力正常」）
    setSt('ov-cpu-st', loadWord(s.cpu_usage));
    setSt('ov-gpu-st', loadWord(s.gpu_usage));
    setSt('ov-mem-st', memWord(s.mem_load));

    // 磁盘：卷列表（见 renderOverviewVolumes）
    renderOverviewVolumes(s);

    // 磁盘负载：读写速率 + 温度
    set('ov-disk-r', fmtRate(s.disk_read_bps));
    set('ov-disk-w', fmtRate(s.disk_write_bps));
    setSt('ov-disk-st', diskWord(s.disk_active));
    setMeta('ov-disk-t', tempText(s.disk_temp_c), s.disk_temp_c >= 55);
    $('ov-disk-t').title = s.disk_temp_c == null
      ? '磁盘温度需要驱动支持；可在 CPU 页启用内置管理员采集'
      : '磁盘传感器温度（内置硬件采集）';
    meter($('ov-disk-m'), s.disk_active, COLORS.disk);

    // 网络：上传 / 下载两条线，数值在卡片里单独成行，不压线
    set('ov-net-up', fmtRate(s.net_up_bps));
    set('ov-net-dn', fmtRate(s.net_down_bps));
    setSt('ov-net-st', netWord((s.net_down_bps || 0) + (s.net_up_bps || 0)));
    const adapters = s.adapters || [];
    const nic = adapters.length ? adapters[0].name : null;
    setMeta('ov-net-nic', nic || '—');
    if (nic) $('ov-net-nic').title = adapters.filter((a) => a.down_bps + a.up_bps > 0).map((a) => a.name).join('、') || nic;

    // 电池：块状电量格 + 大号读数
    const bat = s.battery || {};
    const hasBat = bat.present !== false && bat.percent != null;
    const batPct = hasBat ? bat.percent : 0;
    bigPct('ov-bat-v', hasBat ? batPct : null);
    const batV = $('ov-bat-v');
    if (batV) batV.style.color = hasBat && batPct <= 15 && !bat.charging ? '#d83b3b' : '';
    batteryBlocks($('ov-bat-blk'), batPct, batColor(bat, batPct), hasBat && bat.charging);
    setSt('ov-bat-st', !hasBat ? ['未检测到', ''] : bat.charging ? ['充电中', 'ok'] : bat.ac_line ? ['已接通', ''] : ['放电中', '']);
    const batBits = [];
    if (hasBat) {
      if (bat.remaining_sec != null) {
        batBits.push((bat.charging ? '充满还需 ' : '剩余 ') + fmtHM(bat.remaining_sec));
      }
      if (bat.design_capacity_mwh && bat.full_capacity_mwh) {
        batBits.push('健康 ' + Math.round((bat.full_capacity_mwh / bat.design_capacity_mwh) * 100) + '%');
      }
    }
    set('ov-bat-s', !hasBat ? '无电池' : batBits.join(' · ') || (bat.ac_line ? '已接通电源' : '—'));

    sparkline($('sp-cpu'), hist.cpu, COLORS.cpu, 100);
    sparkline($('sp-gpu'), hist.gpu, COLORS.gpu, 100);
    sparkline($('sp-mem'), hist.mem, COLORS.mem, 100);
    dualChart($('sp-net'), hist.netDown, hist.netUp, COLORS.down, COLORS.up, 64 * 1024, 16 * 1024);
    dualChart($('sp-disk'), hist.diskR, hist.diskW, COLORS.down, COLORS.disk, 64 * 1024, 32 * 1024);

    renderTopProcTable('ov-proc-body', s.procs, 'cpu', 5, { bar: 'cpu' });
    }

    // 硬件采集状态在 CPU 页和设置页都有，两边都要跟着更新。
    if (on('cpu', 'settings')) {
      set('hardware-status', s.hardware_status?.message || '等待内置传感器数据');
      for (const id of ['cpu-enable-sensors', 'settings-enable-sensors']) {
        const button = $(id);
        if (button && !button.dataset.busy) {
          const status = s.hardware_status || {};
          button.disabled = (!!status.admin && !status.retryable) || !!status.pending || status.available === false || demoMode;
          button.textContent = status.pending ? '正在启动或等待授权…' : status.retryable ? '重试温度采集' : status.admin ? '管理员采集已启用' : '管理员启用温度采集';
        }
      }
    }

    // CPU
    if (on('cpu')) {
    set('cpu-bm', fmtPct(s.cpu_usage));
    set('cpu-bs', s.cpu_name || '未知 CPU');
    set('cpu-temp', tempValue(s.cpu_temp_c));
    const cpuTempNote = tempValue(s.cpu_temp_c) !== '—'
      ? '温度来源：' + (s.cpu_temp_source || '实时传感器')
      : s.hardware_status?.pending || s.hardware_status?.retryable ? s.hardware_status.message
      : s.hardware_status?.admin ? '管理员采集已启用，但硬件暂未提供 CPU 温度。'
      : s.hardware_status?.message || '等待内置传感器数据';
    set('cpu-temp-note', cpuTempNote);
    $('ov-cpu-t').title = cpuTempNote;
    set('cpu-cores', s.core_count);
    set('cpu-base', s.cpu_base_mhz ? (s.cpu_base_mhz / 1000).toFixed(2) + ' GHz' : '—');
    set('cpu-procs', s.proc_count);
    set('cpu-uptime', fmtDuration(s.uptime_sec));
    sparkline($('ch-cpu'), hist.cpu, COLORS.cpu, 100);
    renderCores(s.cores, s.core_count);
    renderTopProcTable('cpu-proc-body', s.procs, 'cpu', 8, { pid: true, bar: 'cpu' });
    }

    // 显卡
    if (on('gpu')) {
    set('gpu-bm', fmtPct(s.gpu_usage));
    set('gpu-bs', gpuName + (gpuMem ? ' · ' + gpuMem : ''));
    set('gpu-temp', tempValue(s.gpu_temp_c));
    set('gpu-temp-note', tempValue(s.gpu_temp_c) === '—' ? '驱动或传感器未提供温度' : '主显卡 · 实时温度');
    set('gpu-util', fmtPct(s.gpu_usage));
    set('gpu-ded', (s.gpu_mem_used_gb || 0).toFixed(1) + ' / ' + (s.gpu_mem_total_gb || 0).toFixed(1) + ' GB');
    sparkline($('ch-gpu'), hist.gpu, COLORS.gpu, 100);
    renderGpus(s.gpus);
    }

    // 内存
    if (on('mem')) {
    set('mem-bm', fmtPct(s.mem_load));
    // 容量升为标题行主读数，品牌和频率占用原来的小字位。
    setHtml('mem-cap', window.WinMeterMem.capMarkup(s));
    set('mem-bs', window.WinMeterMem.specText(s));
    renderMemBar(s);
    set('mem-commit', fmtGB(s.commit_gb) + ' / ' + fmtGB(s.commit_limit_gb));
    set('mem-pool', fmtGB(s.pool_paged_gb));
    set('mem-swap', fmtGB(s.swap_used_gb));
    set('mem-swap-s', s.swap_total_gb == null ? '系统未提供' : s.swap_total_gb > 0
      ? '页面文件共 ' + fmtGB(s.swap_total_gb)
      : '未启用页面文件');
    renderTopProcTable('mem-proc-body', s.procs, 'mem', 8, { cpu: false, bar: 'mem', barColor: '#0f9d58', total: (s.mem_total_gb || 0) * 1024 ** 3 });
    }

    // 网络
    if (on('net')) renderNet(s);

    // 磁盘
    if (on('disk')) {
    set('disk-bm', fmtPct(s.disk_active));
    set('disk-read', fmtRate(s.disk_read_bps));
    set('disk-write', fmtRate(s.disk_write_bps));
    set('disk-today-write', fmtBytes(s.disk_today_write));
    window.WinMeterDisk.render(s, {bytes:fmtBytes, cap:fmtCap});
    renderDiskLedger(s, now);
    renderVolumes(s.volumes);
    }

    // 电池 / 蓝牙 / 本机信息
    if (on('battery')) renderBattery(s, now);
    if (on('bt')) renderBluetooth(s.bluetooth);
    if (on('info')) renderInfo(s);

  }

  // ---------- 浏览器预览用演示数据 ----------
  function demoData() {
    const now = Math.floor(Date.now() / 1000);
    const hourK = hourKeyOf(now);
    const dayK = dayKeyOf(now);
    const GB = 1024 ** 3, MB = 1024 ** 2;
    const rnd = (a, b) => a + Math.random() * (b - a);

    const procs = [
      { name: 'chrome.exe', pid: 4820, cpu: rnd(11, 15), mem_bytes: 1.8 * GB, disk_read_bps: 6.8 * MB, disk_write_bps: 9.1 * MB, disk_ops: 842, gpu_percent: 4.1 },
      { name: 'MsMpEng.exe', pid: 2844, cpu: rnd(7, 10), mem_bytes: 412 * MB, disk_read_bps: 31.2 * MB, disk_write_bps: 0.4 * MB, disk_ops: 1204, gpu_percent: null },
      { name: 'devenv.exe', pid: 7712, cpu: rnd(5, 7), mem_bytes: 962 * MB, disk_read_bps: 1.1 * MB, disk_write_bps: 2.4 * MB, disk_ops: 96, gpu_percent: null },
      { name: 'msedgewebview2.exe', pid: 3392, cpu: rnd(4, 6), mem_bytes: 724 * MB, disk_read_bps: 0.6 * MB, disk_write_bps: 1.9 * MB, disk_ops: 61, gpu_percent: 2.2 },
      { name: 'steam.exe', pid: 9016, cpu: rnd(2, 4), mem_bytes: 588 * MB, disk_read_bps: 0.2 * MB, disk_write_bps: 0.8 * MB, disk_ops: 24, gpu_percent: 18.4 },
      { name: 'explorer.exe', pid: 1204, cpu: rnd(1, 3), mem_bytes: 318 * MB, disk_read_bps: 0.1 * MB, disk_write_bps: 0.3 * MB, disk_ops: 12, gpu_percent: null },
    ];
    const hours = [];
    for (let i = 0; i < 14; i++) {
      hours.push({ ts: hourK - i * 3600, read: rnd(1, 8) * GB, write: rnd(0.4, 3) * GB });
    }
    const days = [];
    for (let i = 0; i < 18; i++) {
      if (i % 7 === 3) continue;
      days.push({ ts: dayK - i * 86400, read: rnd(2, 9) * GB, write: rnd(1, 4) * GB });
    }
    const batteryHistory = [];
    for (let i = 0; i < 72; i++) {
      const t = now - (72 - i) * 600;
      const ac = i < 20;
      batteryHistory.push({ ts: t, pct: ac ? 100 : 100 - (i - 20) * 0.9, ac });
    }

    return {
      cpu_usage: rnd(28, 42), cores: Array.from({ length: 16 }, () => rnd(8, 88)), core_count: 16,
      cpu_name: 'Intel Core i7-13700H', cpu_base_mhz: 2400,
      gpu_usage: rnd(38, 55), gpu_name: 'NVIDIA GeForce RTX 4060 Laptop GPU', gpu_mem_used_gb: 4.2, gpu_mem_total_gb: 8.0,
      gpus: [
        { name: 'NVIDIA GeForce RTX 4060 Laptop GPU', usage: rnd(38, 55), mem_used_gb: 4.2, mem_total_gb: 8.0, discrete: true },
        { name: 'Intel(R) Iris(R) Xe Graphics', usage: rnd(1, 9), mem_used_gb: 0.07, mem_total_gb: 0.125, discrete: false },
      ],
      mem_total_gb: 32, mem_used_gb: 12.4, mem_load: 39,
      mem_in_use_gb: 10.9, mem_standby_gb: 3.6, mem_modified_gb: 0.6, mem_cached_gb: 4.8, mem_available_gb: 15.1,
      commit_gb: 18.2, commit_limit_gb: 37.4, pool_paged_gb: 1.1,
      net_down_bps: 2.5 * MB, net_up_bps: 164 * 1024,
      net_session_down_bytes: 3.49 * GB, net_session_up_bytes: 163 * MB,
      adapters: [
        { name: '以太网', down_bps: 2.48 * MB, up_bps: 161 * 1024 },
        { name: 'WLAN', down_bps: 24 * 1024, up_bps: 3 * 1024 },
        { name: 'vEthernet (WSL)', down_bps: 0, up_bps: 0 },
      ],
      net: {
        local_ip: '192.168.31.14', ipv6: '2408:8248:1c22:fcd1::2ff',
        if_name: 'WLAN', mac: '44:F7:9F:A4:42:17', link_mbps: 1441, kind: 'Wi-Fi',
        gateways: ['192.168.31.1'], dns: ['192.168.31.1', '223.5.5.5'],
        wifi_ssid: '枕头包_5G', wifi_signal: 77, wifi_rate: 1441,
        proxy_enabled: false, proxy_server: null,
        egress_ip: '203.0.113.18', egress_region: '中国 福建 厦门',
        egress_isp: '联通', egress_via_proxy: false, egress_at: 0,
        proxy_ip:'198.51.100.24',proxy_path:'tun',proxy_at:0,pac_url:null,
        interfaces:[
          {index:11,name:'WLAN',description:'MediaTek Wi-Fi 7',kind:'Wi-Fi',up:true,is_tun:false,ipv4:'192.168.31.14',ipv6:'2408:8248:1c22:fcd1::2ff',mtu:1500,dns:['192.168.31.1']},
          {index:5,name:'Meta',description:'Meta Tunnel',kind:'其他',up:true,is_tun:true,ipv4:'198.18.0.1',mtu:65535,dns:['198.18.0.2']},
        ],
        routes:['223.5.5.5','1.1.1.1'].map(target=>({target,interface_index:5,if_name:'Meta',next_hop:'198.18.0.2',source:'198.18.0.1',prefix_len:0,is_tun:true})),
        rtt_ms: 38, rtt_target: '223.5.5.5',
      },
      disk_active: 62,
      disk_read_bps: 42 * MB, disk_write_bps: 18 * MB,
      disk_today_read: 42.1 * GB, disk_today_write: 18.4 * GB,
      disk_hours: hours, disk_days: days,
      app_io: Object.fromEntries(['today', 'week', 'month'].map((period, index) => [period, [
        {name:'chrome.exe',path:'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',read:8 * GB,write:3.2 * GB * (index + 1)},
        {name:'steam.exe',path:'C:\\Program Files (x86)\\Steam\\steam.exe',read:2 * GB,write:1.8 * GB * (index + 1)}
      ]])),
      physical_disks: [{id:'/storage/0',number:0,name:'NVMe SSD · 演示设备',kind:'NVMe SSD',bus:'NVMe',firmware:'DEMO',capacity_bytes:1024 ** 4,mounts:['C:','D:'],status:'Good',temperature_c:41,life_percent:98,host_write_gb:18700,host_read_gb:24500,power_on_hours:2130}],
      volumes: [
        { mount: 'C:', label: 'NVMe', free_bytes: 128 * GB, total_bytes: 512 * GB },
        { mount: 'D:', label: 'Data', free_bytes: 1.4 * 1024 ** 4, total_bytes: 2.0 * 1024 ** 4 },
      ],
      battery: {
        present: true, charging: false, ac_line: false, percent: 78, remaining_sec: 13320, rate_w: 14.2,
        power_w: -14.2, power_sensor: true, remain_est: false,
        design_capacity_mwh: 52000, full_capacity_mwh: 47200, cycle_count: 214,
        name: 'DELL 5XV4F', chemistry: '锂离子',
      },
      battery_history: batteryHistory,
      bluetooth: {
        connected: 3, remembered: 7, adapter: 'Realtek RTL8852BE', radio: 'DESKTOP-DEMO',
        devices: [
          { name: 'MX Master 3S', connected: true, remembered: true, kind: '鼠标 · HID', last_used_sec: 120 },
          { name: 'Magic Keyboard', connected: true, remembered: true, kind: '键盘 · HID', last_used_sec: 5 },
          { name: 'Galaxy Buds2 Pro', connected: true, remembered: true, kind: '音频 · 耳机', last_used_sec: 300 },
          { name: 'WH-1000XM5', connected: false, remembered: true, kind: '音频 · 耳机', last_used_sec: 9000 },
          { name: 'iPhone', connected: false, remembered: true, kind: '手机', last_used_sec: 86400 * 3 },
        ],
      },
      power_plan: '平衡', swap_used_gb: 6.2, swap_total_gb: 12,
      procs, uptime_sec: 8047, proc_count: 214,
      cpu_temp_c: 62, gpu_temp_c: 54, disk_temp_c: 41,
      mb_temp_c: 35, fan_rpm: 1234, sensor_src: 'lhm',
      sys: {
        hostname: 'DESKTOP-DEMO', user: 'demo',
        os_name: 'Windows 11 Pro', os_version: '24H2', build: '26100.2894', arch: 'AMD64',
        board: 'ASUSTeK ROG STRIX B650E-F', bios: 'American Megatrends 2201',
        cpu_physical_cores: 16, cpu_logical_cores: 24,
        boot_ts: now - 8047,
        mem_slots: 4,
        mem_modules: [
          { brand: 'Samsung', part: 'M323R2GA3BB0-CQK', slot: 'DIMM_A1', capacity_gb: 16, speed_mts: 4800, kind: 'DDR5', form: 'DIMM' },
          { brand: 'Samsung', part: 'M323R2GA3BB0-CQK', slot: 'DIMM_B1', capacity_gb: 16, speed_mts: 4800, kind: 'DDR5', form: 'DIMM' },
        ],
      },
    };
  }

  function seedDemoHist() {
    const fill = (k, f) => { for (let i = 0; i < HIST_LEN; i++) hist[k].push(f()); };
    fill('cpu', () => 22 + Math.random() * 26);
    fill('gpu', () => 30 + Math.random() * 28);
    fill('mem', () => 37 + Math.random() * 5);
    fill('diskR', () => (24 + Math.random() * 40) * 1024 * 1024);
    fill('diskW', () => (6 + Math.random() * 18) * 1024 * 1024);
    fill('netDown', () => (0.8 + Math.random() * 2.4) * 1024 * 1024);
    fill('netUp', () => (60 + Math.random() * 180) * 1024);
  }

  // ---------- 启动 ----------
  let lastSampleAt = 0;
  let demoMode = false;
  async function start() {
    initNav();
    initTheme();
    initWindowControls();
    window.addEventListener('winmeter-settings', (event) => {
      const next = Math.max(2, Math.round(120000 / event.detail.sample_interval_ms));
      if (next !== HIST_LEN) Object.values(hist).forEach((values) => values.splice(0));
      HIST_LEN = next;
    });

    // Live data comes first. A slow or stuck settings read used to run before
    // this block, which left the window sitting on an empty dashboard forever
    // whenever the settings command did not answer.
    const api = window.__TAURI__;
    if (api && api.event && api.event.listen) {
      lastSampleAt = Date.now();
      try {
        await api.event.listen('sample', (e) => {
          try { render(e.payload); } catch (error) { console.error('[WinMeter] 渲染失败', error); }
        });
        await api.event.listen('navigate', (e) => goPage(e.payload));
      } catch (error) { set('sample-status', '采样连接失败'); $('sample-status').dataset.state = 'error'; }
      setInterval(() => {
        // The settings module can be missing when its script failed to load;
        // fall back to the default interval instead of throwing every second.
        const interval = window.WinMeterSettings?.current?.sample_interval_ms || 1000;
        if (lastSampleAt && Date.now() - lastSampleAt > Math.max(10000, interval * 3)) {
          set('sample-status', '采样暂时中断'); $('sample-status').dataset.state = 'error';
        }
      }, 1000);
    } else {
      demoMode = true;
      console.log('[WinMeter] 未检测到 Tauri 环境，运行演示模式');
      seedDemoHist();
      render(demoData());
      setInterval(() => render(demoData()), 1000);
    }

    // Settings apply afterwards and never hold the dashboard hostage: three
    // seconds is enough for the retry loop, after that the window keeps the
    // defaults instead of staying blank.
    try {
      await Promise.race([
        window.WinMeterSettings.init(),
        new Promise((resolve) => setTimeout(resolve, 3000)),
      ]);
    } catch (error) { console.error('[WinMeter] 设置加载失败', error); }
    window.WinMeterSensorService?.init();
    for (const id of ['cpu-enable-sensors', 'settings-enable-sensors']) {
      const button = $(id);
      if (button) button.onclick = async () => {
        let nextStatus=null;
        button.dataset.busy = 'true'; button.disabled = true; button.textContent = '等待 Windows 授权…';
        try {
          const status = await window.__TAURI__.core.invoke('enable_hardware');
          nextStatus=status;
          set('hardware-status', status.message); set('cpu-temp-note', status.message);
        } catch (error) {set('hardware-status', String(error)); set('cpu-temp-note', String(error));}
        finally {
          delete button.dataset.busy;
          button.disabled=!!nextStatus?.pending || (!!nextStatus?.admin&&!nextStatus?.retryable);
          button.textContent=nextStatus?.pending ? '采集器正在启动…' : nextStatus?.admin&&!nextStatus?.retryable ? '管理员采集已启用' : '重试温度采集';
        }
      };
    }

    // 调试用：?p=cpu 直接打开某一屏（截图 / 演示）
    const m = /[?&]p=([a-z]+)/.exec(location.search);
    if (m) goPage(m[1]);
    if (/[?&]t=dark/.test(location.search)) $('btnTheme').click();
  }

  // Startup must never fail silently: a rejected promise here used to leave the
  // dashboard frozen on its skeleton with nothing in the console to explain it.
  const boot = () => start().catch((error) => {
    console.error('[WinMeter] 启动失败', error);
    set('sample-status', '启动异常'); const badge = $('sample-status'); if (badge) badge.dataset.state = 'error';
  });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
  else boot();
})();
