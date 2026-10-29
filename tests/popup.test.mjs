import test from 'node:test';
import assert from 'node:assert/strict';
import { bytes, rate, pct, temperature, chartPath } from '../src/popup-model.js';

test('missing hardware data is distinct from valid zero readings', () => {
  for (const value of [null, undefined, NaN, Infinity]) {
    assert.equal(pct(value), '—'); assert.equal(temperature(value), '—');
    assert.equal(bytes(value), '—'); assert.equal(rate(value), '—');
  }
  assert.equal(temperature(0), '0 °C'); assert.equal(pct(0), '0%');
  assert.equal(rate(0), '0 B/s'); assert.equal(bytes(1024 ** 4), '1.0 TB');
  assert.equal(pct(120), '100%'); assert.equal(rate(5.7 * 1024), '5.7 KB/s');
});
test('charts retain actual sample spacing and break across sleep gaps', () => {
  const points = [{ at: 0, cpu: 50 }, { at: 60000, cpu: 40 }, { at: 61000, cpu: 30 }];
  const { line, area } = chartPath(points, 'cpu', 100, 120000);
  assert.equal((line.match(/M/g) || []).length, 2);
  assert.match(line, /^M0\.00,40\.00M120\.00,/);
  assert.equal((area.match(/Z/g) || []).length, 1);
});
test('charts exclude expired, future, and missing values without fabricating zero', () => {
  assert.deepEqual(chartPath([{ at: -1, cpu: 20 }, { at: 120001, cpu: 20 }, { at: 60000, cpu: null }], 'cpu', 100, 120000), { line: '', area: '' });
  const graph = chartPath([{ at: 120000, cpu: 10000 }], 'cpu', 100, 120000);
  assert.equal(graph.line, 'M240.00,2.00'); assert.equal(graph.area, '');
});
