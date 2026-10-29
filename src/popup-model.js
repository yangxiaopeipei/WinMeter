export const valid = value => typeof value === 'number' && Number.isFinite(value);
export const percent = value => valid(value) ? Math.max(0, Math.min(100, value)) : null;
export const pct = value => valid(value) ? `${Math.round(percent(value))}%` : '—';
export const temperature = value => valid(value) ? `${Math.round(value)} °C` : '—';
export function bytes(value) {
  if (!valid(value) || value < 0) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return `${value.toFixed(index >= 3 ? 1 : 0)} ${units[index]}`;
}
export function rate(value) {
  if (!valid(value) || value < 0) return '—';
  const units = ['B/s', 'KB/s', 'MB/s', 'GB/s', 'TB/s'];
  let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return `${value.toFixed(index && value < 100 ? 1 : 0)} ${units[index]}`;
}
export function chartPath(points, key, max, now, width = 240, height = 80) {
  const usable = points.filter(p => valid(p.at) && p.at >= now - 120000 && p.at <= now && valid(p[key]));
  if (!usable.length) return { line: '', area: '' };
  const limit = valid(max) && max > 0 ? max : 1;
  const coordinates = usable.map(p => [
    ((p.at - now + 120000) / 120000 * width).toFixed(2),
    (height - 2 - Math.min(limit, Math.max(0, p[key])) / limit * (height - 4)).toFixed(2),
  ]);
  let line = ''; let previous;
  const segments = [];
  for (let i = 0; i < coordinates.length; i++) {
    // Do not draw an invented continuous history across sleep or sampling gaps.
    if (!previous || usable[i].at - previous.at > 15000) {
      segments.push([]); line += `M${coordinates[i].join(',')}`;
    } else line += `L${coordinates[i].join(',')}`;
    segments.at(-1).push(coordinates[i]); previous = usable[i];
  }
  const area = segments.filter(s => s.length > 1).map(s =>
    `M${s[0][0]},${height}L${s.map(p => p.join(',')).join('L')}L${s.at(-1)[0]},${height}Z`).join('');
  return { line, area };
}
