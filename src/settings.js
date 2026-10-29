// Preference controls, with a browser-only preview fallback.
(function () {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const defaults = {
    theme: 'system', font_family: 'Segoe UI', font_size: 13, sample_interval_ms: 1000,
    close_to_tray: true, minimize_to_tray: true, launch_at_startup: false, hardware_auto_admin: true, taskbar_enabled: false, taskbar_network: true,
    taskbar_cpu: true, taskbar_memory: true, taskbar_cpu_temp: false, taskbar_gpu_temp: false,
    taskbar_disk_temp: false, taskbar_text_align: 'left', taskbar_font: 'Segoe UI',
    taskbar_font_size: 12, taskbar_position: 'right', taskbar_offset: 0, taskbar_theme: 'system',
    taskbar_gap: 4, taskbar_order: ['network', 'cpu', 'memory', 'cpu_temp', 'gpu_temp', 'disk_temp'],
  };
  const fields = {
    theme: 'theme', font_family: 'font', font_size: 'font-size', sample_interval_ms: 'interval',
    close_to_tray: 'close-tray', minimize_to_tray: 'minimize-tray', taskbar_enabled: 'taskbar-enabled', taskbar_network: 'taskbar-network',
    launch_at_startup: 'startup',
    hardware_auto_admin: 'hardware-auto-admin',
    taskbar_cpu: 'taskbar-cpu', taskbar_memory: 'taskbar-memory', taskbar_font: 'taskbar-font',
    taskbar_font_size: 'taskbar-font-size', taskbar_position: 'taskbar-position',
    taskbar_offset: 'taskbar-offset', taskbar_theme: 'taskbar-theme',
    taskbar_gap: 'taskbar-gap', taskbar_cpu_temp: 'taskbar-cpu-temp', taskbar_gpu_temp: 'taskbar-gpu-temp',
    taskbar_disk_temp: 'taskbar-disk-temp', taskbar_text_align: 'taskbar-text-align',
  };
  let saved = { ...defaults };
  let pending = { ...defaults };
  let sample = null;
  let writes = Promise.resolve();
  let settingsLoaded = false;
  const enqueue = (action) => { const result = writes.then(action); writes = result.catch(() => {}); return result; };
  const native = () => !!window.__TAURI__?.core?.invoke;
  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
  const systemDark = window.matchMedia('(prefers-color-scheme: dark)');
  const fontFields = ['setting-font', 'setting-taskbar-font'];
  function ensureFont(select, name) {
    if (!Array.from(select.options).some((option) => option.value === name)) {
      const option = document.createElement('option'); option.value = name; option.textContent = name;
      select.append(option);
    }
  }
  async function loadFonts() {
    const hint = $('system-font-status');
    let names = ['Segoe UI','Microsoft YaHei UI','Microsoft YaHei','SimSun','Consolas','Cascadia Mono'];
    try {
      if (native()) {
        names = await invoke('get_system_fonts');
        hint.textContent = '已读取 ' + names.length + ' 种系统字体，选择后即时预览';
      } else hint.textContent = '浏览器预览仅列常用字体，程序内读取系统字体';
    } catch { hint.textContent = '系统字体读取失败，暂用常用字体与当前设置'; }
    fontFields.forEach((id) => {
      const select = $(id), current = select.value;
      select.replaceChildren();
      [...new Set([...names, current].filter(Boolean))].sort((a,b) => a.localeCompare(b,'zh-CN')).forEach((name) => ensureFont(select, name));
      select.value = current;
    });
  }

  function apply(s) {
    const dark = s.theme === 'dark' || (s.theme === 'system' && systemDark.matches);
    $('win').dataset.t = dark ? 'dark' : 'light';
    // 根节点也要跟着换，否则 html 的兜底底色和页面主题对不上。
    document.documentElement.dataset.t = dark ? 'dark' : 'light';
    // 记一份，下次启动 index.html 里的内联脚本能在首帧前就用对底色，不闪白/闪黑。
    try { localStorage.setItem('winmeter-theme', dark ? 'dark' : 'light'); } catch (e) {}
    // 资源速览是独立窗口，只认「已保存」的设置。而设置页选主题是即时预览、
    // 要按保存才落盘，于是会出现主界面已经变深、弹窗还是浅的。这里把当前
    // 生效的深浅色直接广播过去，让两个窗口看到的是同一个主题。
    try { if (native()) window.__TAURI__.event.emit('theme-preview', { dark }); } catch (e) {}
    $('win').style.setProperty('--font', JSON.stringify(s.font_family) + ', "Microsoft YaHei UI", system-ui, sans-serif');
    $('win').style.setProperty('--font-scale', s.font_size / 13);
    $('themeIco').setAttribute('href', dark ? '#i-sun' : '#i-moon');
    $('themeLbl').textContent = dark ? '浅色' : '深色';
    const interval = saved.sample_interval_ms / 1000;
    $('sampling-label').textContent = '每 ' + interval + ' 秒更新';
    $('net-range').textContent = '最近 2 分钟 · 每 ' + interval + ' 秒采样';
    previewTaskbar();
  }
  function normalize(s) {
    const order = s.taskbar_order || defaults.taskbar_order;
    return { ...defaults, ...s, taskbar_order: [...order, ...defaults.taskbar_order.filter(key => !order.includes(key))] };
  }
  function fill(s) {
    s = normalize(s);
    pending = { ...s, taskbar_order: [...(s.taskbar_order || defaults.taskbar_order)] };
    ensureFont($('setting-font'), s.font_family);
    ensureFont($('setting-taskbar-font'), s.taskbar_font);
    Object.entries(fields).forEach(([key, name]) => {
      const el = $('setting-' + name);
      if (typeof defaults[key] === 'boolean') el.checked = s[key];
      else el.value = s[key];
    });
    renderOrder(); apply(s);
  }
  function read() {
    const s = {};
    Object.entries(fields).forEach(([key, name]) => {
      const el = $('setting-' + name);
      s[key] = typeof defaults[key] === 'boolean' ? el.checked
        : typeof defaults[key] === 'number' ? Number(el.value) : el.value.trim();
    });
    s.taskbar_order = [...pending.taskbar_order];
    return s;
  }
  function renderOrder() {
    const names = { network: '网速', cpu: 'CPU', memory: '内存', cpu_temp: 'CPU温度', gpu_temp: '显卡温度', disk_temp: '磁盘温度' };
    $('taskbar-order').replaceChildren(...pending.taskbar_order.map((key, index) => {
      const item = document.createElement('div'); item.className = 'order-item';
      const label = document.createElement('span'); label.textContent = names[key]; item.append(label);
      [-1, 1].forEach((direction) => {
        const button = document.createElement('button'); button.type = 'button';
        button.textContent = direction < 0 ? '↑' : '↓';
        button.setAttribute('aria-label', names[key] + (direction < 0 ? '上移' : '下移'));
        button.disabled = index + direction < 0 || index + direction >= pending.taskbar_order.length;
        button.onclick = () => {
          const order = [...pending.taskbar_order];
          [order[index], order[index + direction]] = [order[index + direction], order[index]];
          pending.taskbar_order = order; pending = read(); renderOrder(); apply(pending);
          $('settings-state').textContent = '有未保存的修改';
        };
        item.append(button);
      });
      return item;
    }));
  }
  function speed(bps) {
    let value = Number.isFinite(bps) ? Math.max(0, bps) : 0;
    const units = ['B/s', 'KB/s', 'MB/s', 'GB/s', 'TB/s'];
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
    if (unit === units.length - 1 && value >= 1000) return '999+ TB/s';
    return value.toFixed(unit >= 2 && value < 100 ? 1 : 0) + ' ' + units[unit];
  }
  function temperature(value) {
    return typeof value === 'number' && Number.isFinite(value) ? value.toFixed(0) + ' ℃' : '—';
  }
  function previewTaskbar() {
    const el = $('taskbar-preview');
    if (!el) return;
    const s = pending;
    const dark = s.taskbar_theme === 'dark' || (s.taskbar_theme === 'system' && systemDark.matches);
    el.dataset.t = dark ? 'dark' : 'light';
    el.style.fontFamily = JSON.stringify(s.taskbar_font) + ', system-ui';
    el.style.fontSize = s.taskbar_font_size + 'px';
    el.style.textAlign = s.taskbar_text_align;
    el.style.gap = s.taskbar_gap + 'px';
    el.style.lineHeight = (s.taskbar_font_size + 2) + 'px';
    el.parentElement.dataset.t = dark ? 'dark' : 'light';
    const pairs = [];
    for (const key of s.taskbar_order) {
      if (key === 'network' && s.taskbar_network) pairs.push([speed(sample?.net_up_bps), speed(sample?.net_down_bps), true]);
      if (key === 'cpu' && s.taskbar_cpu) pairs.push(['CPU', sample ? sample.cpu_usage.toFixed(0) + '%' : '—', false, '100%']);
      if (key === 'memory' && s.taskbar_memory) pairs.push(['内存', sample ? sample.mem_load + '%' : '—', false, '100%']);
      if (key === 'cpu_temp' && s.taskbar_cpu_temp) pairs.push(['CPU温度', temperature(sample?.cpu_temp_c), false, '100 ℃']);
      if (key === 'gpu_temp' && s.taskbar_gpu_temp) pairs.push(['显卡温度', temperature(sample?.gpu_temp_c), false, '100 ℃']);
      if (key === 'disk_temp' && s.taskbar_disk_temp) pairs.push(['磁盘温度', temperature(sample?.disk_temp_c), false, '100 ℃']);
    }
    // Match the native HUD's fixed fields, including the independent arrow
    // column. Readout digit changes must not move neighboring labels.
    const measure = document.createElement('canvas').getContext('2d');
    measure.font = '400 ' + s.taskbar_font_size + 'px ' + JSON.stringify(s.taskbar_font) + ', system-ui';
    const width = (text) => Math.round(measure.measureText(text).width);
    el.style.fontWeight = '400';
    el.replaceChildren(...pairs.map(([top, bottom, network, template]) => {
      const span = document.createElement('span');
      if (network) {
        span.className = 'network-column';
        const arrowWidth = Math.max(width('↑ '), width('↓ '));
        const valueWidth = Math.max(...['999.9 MB/s', '999.9 GB/s', '999.9 KB/s', '999+ TB/s', '1024 B/s'].map(width));
        span.style.width = (arrowWidth + valueWidth) + 'px';
        [top, bottom].forEach((text, index) => {
          const row = document.createElement('span'); row.className = 'network-row';
          row.style.gridTemplateColumns = arrowWidth + 'px ' + valueWidth + 'px';
          const arrow = document.createElement('span'); arrow.textContent = index ? '↓' : '↑';
          const value = document.createElement('span'); value.textContent = text;
          row.append(arrow, value); span.append(row);
        });
      } else {
        span.style.width = Math.max(width(top), width(bottom), width(template || top)) + 'px';
        span.append(document.createTextNode(top), document.createElement('br'), document.createTextNode(bottom));
      }
      span.style.flex = 'none';
      return span;
    }));
    if (!pairs.length) el.textContent = '请选择显示指标';
  }
  async function save() {
    if (!settingsLoaded) throw new Error('配置尚未加载，暂不能保存');
    const s = read();
    if (s.taskbar_enabled && !s.taskbar_network && !s.taskbar_cpu && !s.taskbar_memory && !s.taskbar_cpu_temp && !s.taskbar_gpu_temp && !s.taskbar_disk_temp) {
      throw new Error('请至少选择一个任务栏指标');
    }
    if (!s.font_family || !s.taskbar_font) throw new Error('字体名称不能为空');
    await enqueue(async () => {
      saved = native() ? await invoke('save_settings', { settings: s }) : s;
    });
    if (!native()) localStorage.setItem('winmeter-preview-settings', JSON.stringify(saved));
    fill(saved);
    $('settings-state').textContent = native() ? '已保存，下次启动自动恢复' : '已保存浏览器预览设置';
    window.dispatchEvent(new CustomEvent('winmeter-settings', { detail: saved }));
  }
  async function refreshHudStatus() {
    if (!native()) { $('taskbar-status').textContent = '浏览器仅支持预览'; return; }
    try {
      const status = await invoke('taskbar_status');
      $('taskbar-status').textContent = status.message || '等待任务栏连接';
      $('taskbar-status').classList.toggle('connected', status.attached);
    } catch { $('taskbar-status').textContent = '任务栏状态暂不可用'; }
  }
  window.WinMeterSettings = {
    get current() { return saved; },
    // 任务栏预览只有设置页看得见，别的页每秒重建它纯属浪费。
    renderSample(s, visible) { sample = s; if (visible) previewTaskbar(); },
    async toggleTheme() {
      if ($('settings-save').disabled) return;
      const dark = $('win').dataset.t === 'dark';
      pending.theme = dark ? 'light' : 'dark';
      $('setting-theme').value = pending.theme;
      apply(pending);
      $('settings-state').textContent = '有未保存的修改';
      // A title-bar theme change persists only the theme, preserving unsaved form edits.
      try {
        const theme = pending.theme;
        await enqueue(async () => {
          const next = { ...saved, theme };
          saved = native() ? await invoke('save_settings', { settings: next }) : next;
        });
        if (!native()) localStorage.setItem('winmeter-preview-settings', JSON.stringify(saved));
        const current = read();
        if (Object.keys(saved).every(key => JSON.stringify(current[key]) === JSON.stringify(saved[key]))) $('settings-state').textContent = '已保存，下次启动自动恢复';
      } catch (error) { $('settings-state').textContent = '主题保存失败：' + error; }
    },
    async init() {
      $('settings-save').disabled = true;
      try {
        saved = native() ? await window.WinMeterSettingsIO.load(invoke)
          : { ...defaults, ...JSON.parse(localStorage.getItem('winmeter-preview-settings') || '{}') };
        settingsLoaded = true;
      } catch (error) { $('settings-state').textContent = '配置未加载，已禁用保存以保留原配置：' + error; }
      $('settings-save').disabled = !settingsLoaded;
      saved = normalize(saved);
      fill(saved);
      void loadFonts();
      if (settingsLoaded) window.dispatchEvent(new CustomEvent('winmeter-settings', { detail: saved }));
      $('settings-form').addEventListener('input', () => {
        if (!settingsLoaded) return;
        pending = read(); apply(pending); $('settings-state').textContent = '有未保存的修改';
      });
      $('settings-form').addEventListener('submit', async (event) => {
        event.preventDefault(); $('settings-save').disabled = true;
        try { await save(); } catch (error) { $('settings-state').textContent = String(error.message || error); }
        finally { $('settings-save').disabled = !settingsLoaded; refreshHudStatus(); }
      });
      $('settings-defaults').disabled = !settingsLoaded;
      $('settings-defaults').onclick = () => { fill(defaults); $('settings-state').textContent = '已恢复默认预览，点击保存生效'; };
      systemDark.addEventListener('change', () => apply(pending));
      refreshHudStatus(); setInterval(refreshHudStatus, 3000);
    },
  };
})();
