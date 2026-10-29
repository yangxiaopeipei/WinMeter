(function () {
  'use strict';
  // WebViews can invoke commands before Tauri has registered managed state.
  // Retry only that startup condition; never replace a failed read with defaults.
  window.WinMeterSettingsIO = {
    async load(invoke, sleep = ms => new Promise(resolve => setTimeout(resolve, ms))) {
      for (let attempt = 0; ; attempt++) {
        try { return await invoke('get_settings'); }
        catch (error) {
          if (!/state not managed/i.test(String(error)) || attempt >= 20) throw error;
          await sleep(150);
        }
      }
    },
  };
})();
