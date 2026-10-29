const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function mem() {
  const context = { window: {} };
  vm.runInNewContext(fs.readFileSync('src/mem.js', 'utf8'), context);
  return context.window.WinMeterMem;
}

test('capacity keeps the used figure large and demotes the total to a suffix', () => {
  const m = mem();
  assert.equal(m.capMarkup({ mem_used_gb: 10.04, mem_total_gb: 15.4 }),
    '10.0<span class="u"> / 15.4 GB</span>');
  assert.equal(m.capMarkup({ mem_used_gb: null, mem_total_gb: 15.4 }), '—');
  assert.equal(m.capMarkup({}), '—');
});

test('capacity switches to TB above 1 TB instead of printing four-digit GB', () => {
  const m = mem();
  assert.equal(m.capMarkup({ mem_used_gb: 300, mem_total_gb: 2048 }),
    '0.29<span class="u"> / 2.00 TB</span>');
});

test('module spec shows brand with type and speed', () => {
  const m = mem();
  const sample = {
    sys: {
      mem_modules: [
        { brand: 'Micron', kind: 'DDR5', speed_mts: 6400, capacity_gb: 16 },
        { brand: null, kind: null, speed_mts: null, capacity_gb: 0 },
      ],
    },
  };
  assert.equal(m.specText(sample), 'Micron DDR5-6400');
});

test('identical modules collapse into a count and mixed modules are listed side by side', () => {
  const m = mem();
  const same = { sys: { mem_modules: [
    { brand: 'Micron', kind: 'DDR5', speed_mts: 6400, capacity_gb: 16 },
    { brand: 'Micron', kind: 'DDR5', speed_mts: 6400, capacity_gb: 16 },
  ] } };
  assert.equal(m.specText(same), 'Micron DDR5-6400 × 2');

  const mixed = { sys: { mem_modules: [
    { brand: 'Micron', kind: 'DDR5', speed_mts: 6400, capacity_gb: 16 },
    { brand: 'Samsung', kind: 'DDR4', speed_mts: 3200, capacity_gb: 8 },
  ] } };
  assert.equal(m.specText(mixed), 'Micron DDR5-6400 + Samsung DDR4-3200');
});

test('missing firmware data degrades to a dash instead of inventing a reading', () => {
  const m = mem();
  assert.equal(m.specText({}), '—');
  assert.equal(m.specText({ sys: { mem_modules: [{ capacity_gb: 0 }] } }), '—');
  // 品牌读不到但类型和频率还在时，只显示能确认的部分
  assert.equal(m.specText({ sys: { mem_modules: [{ brand: null, kind: 'DDR5', speed_mts: 4800, capacity_gb: 8 }] } }),
    'DDR5-4800');
  assert.equal(m.specText({ sys: { mem_modules: [{ brand: null, kind: null, speed_mts: 4800, capacity_gb: 8 }] } }),
    '4800 MT/s');
});
