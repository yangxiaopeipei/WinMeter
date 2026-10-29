import { readFileSync } from 'node:fs';
const json = (file) => JSON.parse(readFileSync(new URL(`../${file}`, import.meta.url), 'utf8'));
const pkg = json('package.json');
const lock = json('package-lock.json');
if (pkg.version !== lock.version || pkg.version !== lock.packages[''].version) {
  throw new Error('Package lock version mismatch');
}
if (pkg.devDependencies['@tauri-apps/cli'] !== lock.packages[''].devDependencies['@tauri-apps/cli']) {
  throw new Error('Tauri CLI lock mismatch');
}
console.log(pkg.version);