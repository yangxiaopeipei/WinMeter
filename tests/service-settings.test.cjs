const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const { readFileSync } = require('node:fs');
const context = { window: {} };
vm.runInNewContext(readFileSync(require('node:path').join(__dirname, '../src/service-settings.js'), 'utf8'), context);
const view = context.window.WinMeterSensorService.view;
test('missing service only offers install; busy or browser disables operations', () => {
  assert.equal(view({}, false, false).showInstalled, false);
  assert.equal(view({}, true, false).disabled, true);
  assert.equal(view({}, false, true).disabled, true);
});
test('stopped and disabled services offer recovery, running service offers stop', () => {
  assert.equal(view({ installed: true, enabled: true, running: false }).toggleAction, 'resume');
  assert.equal(view({ installed: true, enabled: false, running: false }).toggleAction, 'resume');
  assert.equal(view({ installed: true, enabled: true, running: true }).toggleAction, 'disable');
  assert.equal(view({ installed: true }).showInstalled, true);
});
