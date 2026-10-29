(function () {
  'use strict';

  // 内存页标题行的两段文案。抽成纯函数放在这里，是为了能在 node 里直接
  // 跑测试——main.js 是 IIFE 且依赖整套 DOM，没法单独加载。

  // 大字只强调已用容量，总量降为后缀，避免两个数字抢同一视线。
  function capMarkup(sample) {
    // Number(null) 是 0，直接转会把「没有读数」画成 0.0 GB
    const num = (v) => (v == null || v === '' ? NaN : Number(v));
    const used = num(sample && sample.mem_used_gb);
    const total = num(sample && sample.mem_total_gb);
    if (!Number.isFinite(used) || !Number.isFinite(total)) return '—';
    const T = 1024;
    const big = total >= T;
    const unit = big ? ' TB' : ' GB';
    const scale = big ? T : 1;
    return (used / scale).toFixed(big ? 2 : 1) +
      '<span class="u"> / ' + (total / scale).toFixed(big ? 2 : 1) + unit + '</span>';
  }

  // 品牌 + 类型 + 频率，例如「Micron DDR5-6400」。同规格多条合并成 ×N，
  // 混插则并列。读不到固件信息时返回破折号，由界面决定怎么弱化。
  function specText(sample) {
    const modules = (sample && sample.sys && sample.sys.mem_modules) || [];
    const filled = modules.filter((m) => m && Number(m.capacity_gb) > 0);
    if (!filled.length) return '—';

    const groups = new Map();
    filled.forEach((m) => {
      const brand = String(m.brand || '').trim();
      const speed = m.speed_mts ? String(m.speed_mts) : '';
      const kind = m.kind ? (speed ? m.kind + '-' + speed : m.kind) : (speed ? speed + ' MT/s' : '');
      const label = [brand, kind].filter(Boolean).join(' ');
      const key = label || '内存';
      groups.set(key, (groups.get(key) || 0) + 1);
    });

    return Array.from(groups.entries())
      .map(([label, count]) => (count > 1 ? label + ' × ' + count : label))
      .join(' + ');
  }

  window.WinMeterMem = { capMarkup, specText };
})();
