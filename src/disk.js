(function () {
  'use strict';
  let latest = {}, format;
  const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'})[c]);
  const $ = (id) => document.getElementById(id);
  const present = (v) => v != null && Number.isFinite(Number(v));
  function appRows() {
    const period = $('disk-app-period')?.value || 'today';
    const rows = latest.app_io?.[period] || [];
    const body = $('disk-app-body');
    if (!body) return;
    const max = Math.max(1, ...rows.map((r) => r.write));
    body.innerHTML = rows.length ? rows.map((r) => '<tr><td class="app-identity"><b>' + esc(r.name) + '</b><small title="' + esc(r.path || '') + '">' + esc(r.path || '路径未获取 · 按进程名归组') + '</small></td><td class="r n">' + format.bytes(r.write) + '</td><td class="r">' + format.bytes(r.read) + '</td><td class="disk-share"><div class="app-write-bar" title="相对本表最高写入量"><i style="width:' + Math.max(0, Math.min(100, r.write / max * 100)).toFixed(1) + '%"></i></div></td></tr>').join('') : '<tr><td colspan="4" class="disk-empty">开始记录后会在这里显示应用；首次采样只建立基线。</td></tr>';
  }
  function devices() {
    const body = $('disk-devices');
    if (!body) return;
    const disks = latest.physical_disks || [];
    if (!disks.length) {
      body.innerHTML = '<p class="disk-empty">' + esc(latest.hardware_status?.pending ? '正在读取磁盘信息…' : '尚未获取物理磁盘信息，可在设置中启用管理员硬件采集。') + '</p>';
      return;
    }
    const expanded = new Set(Array.from(body.querySelectorAll('details[open]')).map((d) => d.dataset.disk));
    const html = disks.map((d) => {
      const state = {Good:['正常','ok'], Caution:['需要关注','warn'], Bad:['异常','bad']}[d.status] || ['健康状态未提供',''];
      const metric = (label, value, title = '') => '<div title="' + esc(title) + '"><small>' + label + '</small><b>' + esc(value) + '</b></div>';
      let metrics = '';
      if (present(d.temperature_c)) metrics += metric('温度', Number(d.temperature_c).toFixed(0) + ' °C');
      if (present(d.life_percent)) metrics += metric('预估剩余寿命', Number(d.life_percent).toFixed(0) + '%', '设备固件估算的耐久度，不能换算为还能使用多少天');
      if (present(d.host_write_gb)) metrics += metric('累计主机写入', Number(d.host_write_gb).toLocaleString('zh-CN') + ' GB', '设备自出厂以来记录的主机写入量，包含 WinMeter 未运行期间；不等于 NAND 内部写入量');
      if (present(d.host_read_gb)) metrics += metric('累计主机读取', Number(d.host_read_gb).toLocaleString('zh-CN') + ' GB');
      if (present(d.power_on_hours)) metrics += metric('通电时间', Number(d.power_on_hours).toLocaleString('zh-CN') + ' 小时');
      const meta = ['磁盘 ' + d.number, d.kind, d.capacity_bytes ? format.cap(d.capacity_bytes) : '', (d.mounts || []).join(' / ')].filter(Boolean).map(esc).join(' · ');
      return '<article class="disk-device"><div class="disk-device-title"><div><h4>' + esc(d.name) + '</h4><small>' + meta + '</small></div><span class="badge ' + state[1] + '">' + state[0] + '</span></div>' + (metrics ? '<div class="disk-health-metrics">' + metrics + '</div>' : '<p class="note">设备未提供 SMART 读数；不据此判断健康状态。</p>') + '<details data-disk="' + esc(d.id) + '"' + (expanded.has(d.id) ? ' open' : '') + '><summary>设备详情</summary><div class="note">' + [d.bus ? '接口：' + esc(d.bus) : '', d.firmware ? '固件：' + esc(d.firmware) : ''].filter(Boolean).join(' · ') + '</div></details></article>';
    }).join('');
    if (body.innerHTML !== html) body.innerHTML = html;
  }
  window.WinMeterDisk = {
    render(sample, formatters) {
      latest = sample; format = formatters;
      const period = $('disk-app-period');
      if (period && !period.onchange) period.onchange = appRows;
      appRows(); devices();
    }
  };
})();
