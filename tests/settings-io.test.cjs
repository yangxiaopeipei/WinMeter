const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const vm = require('node:vm');
function loader() {
  const context = { window: {}, setTimeout };
  vm.runInNewContext(readFileSync(require('node:path').join(__dirname, '../src/settings-io.js'), 'utf8'), context);
  return context.window.WinMeterSettingsIO.load;
}
test('startup retries return saved settings unchanged', async () => {
  const saved = { taskbar_font_size: 16, taskbar_gap: 13, theme: 'light' };
  let calls = 0;
  const result = await loader()(async command => {
    assert.equal(command, 'get_settings');
    if (++calls < 3) throw "state not managed for field 'store'";
    return saved;
  }, async () => {});
  assert.equal(result, saved);
  assert.equal(calls, 3);
});
test('permanent failures reject without retry or default settings', async () => {
  let calls = 0;
  await assert.rejects(loader()(async () => { calls++; throw new Error('read failed'); }, async () => {}), /read failed/);
  assert.equal(calls, 1);
});
test('startup retry limit rejects without default settings', async () => {
  let calls = 0;
  await assert.rejects(loader()(async () => { calls++; throw new Error('state not managed'); }, async () => {}), /state not managed/);
  assert.equal(calls, 21);
});
