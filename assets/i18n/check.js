// Check Snagster language packs against assets/i18n/strings.json.
// Usage: node assets/i18n/check.js [code ...]   (no codes: every pack)
// Reports missing texts, empty ones, extra keys, and placeholders ({name}, %Y...) that differ.
const fs = require('fs');
const path = require('path');
const dir = __dirname;
const keys = JSON.parse(fs.readFileSync(path.join(dir, 'strings.json'), 'utf8'));
const names = JSON.parse(fs.readFileSync(path.join(dir, 'language-names.json'), 'utf8'));
let codes = process.argv.slice(2);
if (codes.length === 0) codes = Object.keys(names).filter(c => c !== 'en');
const marks = s => [...s.matchAll(/\{[a-z_]+\}|%[YmdHMSnt]/g)].map(m => m[0]).sort().join(' ');
let bad = 0;
for (const code of codes) {
  const file = path.join(dir, code + '.json');
  let pack;
  try {
    pack = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (e) {
    console.log(`${code}: can't read ${file}: ${e.message}`);
    bad++;
    continue;
  }
  const s = pack.strings || {};
  const problems = [];
  if (pack.code !== code) problems.push(`"code" is ${pack.code}, should be ${code}`);
  for (const k of keys) {
    if (!(k in s)) problems.push(`missing: ${JSON.stringify(k)}`);
    else if (!String(s[k]).trim()) problems.push(`empty: ${JSON.stringify(k)}`);
    else if (marks(k) !== marks(s[k])) problems.push(`placeholders differ in ${JSON.stringify(k)}: ${JSON.stringify(s[k])}`);
  }
  for (const k of Object.keys(s)) if (!keys.includes(k)) problems.push(`extra key: ${JSON.stringify(k)}`);
  const done = keys.filter(k => k in s && String(s[k]).trim()).length;
  console.log(`${code}: ${done}/${keys.length} translated${problems.length ? `, ${problems.length} problems` : ', OK'}`);
  for (const p of problems.slice(0, 25)) console.log('   ' + p);
  if (problems.length) bad++;
}
process.exit(bad ? 1 : 0);
