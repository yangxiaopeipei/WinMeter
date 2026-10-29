(function () {
  'use strict';
  function view(state, busy, demo) {
    return {
      disabled: !!busy || !!demo,
      install: state.installed ? '更新采集服务' : '安装自动采集服务',
      toggle: state.enabled && state.running ? '停用自动采集' : '恢复自动采集',
      toggleAction: state.enabled && state.running ? 'disable' : 'resume',
      showInstalled: !!state.installed,
    };
  }
  window.WinMeterSensorService = {
    view,
    init() {
      const $ = id => document.getElementById(id);
      const invoke = window.__TAURI__?.core?.invoke;
      let state = {}, busy = false, known = false;
      function render() {
        const model = view(state, busy || !known, !invoke);
        $('sensor-service-install').disabled = model.disabled;
        $('sensor-service-install').textContent = model.install;
        $('sensor-service-toggle').disabled = model.disabled;
        $('sensor-service-toggle').textContent = model.toggle;
        $('sensor-service-remove').disabled = model.disabled;
        $('sensor-service-toggle').hidden = !model.showInstalled;
        $('sensor-service-remove').hidden = !model.showInstalled;
        $('setting-hardware-auto-admin').disabled = !!state.installed;
      }
      async function refresh() {
        if (busy) return;
        try {
          if (!invoke) { $('sensor-service-status').textContent = '浏览器仅展示预览，安装需在 WinMeter 内操作'; return; }
          state = await invoke('sensor_service_status'); known = true;
          $('sensor-service-status').textContent = state.message;
        } catch { $('sensor-service-status').textContent = '服务状态读取失败，暂不能操作'; known = false; }
        finally { render(); }
      }
      async function manage(action) {
        if (busy || !known || !invoke) return;
        busy = true; render();
        $('sensor-service-status').textContent = '请处理 Windows 授权提示，正在执行服务操作…';
        try {
          state = await invoke('manage_sensor_service', { action });
          $('sensor-service-status').textContent = action === 'remove' ? '服务已卸载；恢复随程序运行的采集方式' : state.message;
        } catch (error) { $('sensor-service-status').textContent = String(error); }
        finally { busy = false; render(); }
      }
      $('sensor-service-install').onclick = () => manage('install');
      $('sensor-service-toggle').onclick = () => manage(view(state, false, false).toggleAction);
      $('sensor-service-remove').onclick = () => manage('remove');
      render(); void refresh(); setInterval(refresh, 5000);
    },
  };
})();
