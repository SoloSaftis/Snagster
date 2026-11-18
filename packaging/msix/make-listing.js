// Build the Microsoft Store listing import folder (Partner Center > Store listings > Import
// listings > Upload folder) from the translations in listing/<store-code>.json.
//
//   node packaging/msix/make-listing.js [out-folder]     (default: store-listing)
//
// listing-fields.csv is Partner Center's own export (Field, ID and Type must not change); every
// row is kept (trailer rows must stay). Title, publisher, copyright, the screenshots and the logo
// go in the "default" column, shared by all languages; the texts go in one column per language,
// in the order of store-languages.json (English first). The screenshots come from
// assets/screenshots and the logo from `snagster --icon-png <file> 300`, which must have been
// written to <out-folder>/store-logo-300.png first (CI or by hand).
const fs = require('fs');
const path = require('path');
const here = __dirname;
const root = path.resolve(here, '..', '..');
const outDir = path.resolve(process.argv[2] || 'store-listing');

function parseCsv(text) {
  const rows = [];
  let row = [], field = '', q = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (q) {
      if (c === '"' && text[i + 1] === '"') { field += '"'; i++; }
      else if (c === '"') q = false;
      else field += c;
    } else if (c === '"') q = true;
    else if (c === ',') { row.push(field); field = ''; }
    else if (c === '\n') { row.push(field.replace(/\r$/, '')); rows.push(row); row = []; field = ''; }
    else field += c;
  }
  if (field.length || row.length) { row.push(field); rows.push(row); }
  return rows;
}
const cell = v => /[",\r\n]/.test(v) ? '"' + v.replace(/"/g, '""') + '"' : v;

// The Store's limits (characters).
const limit = f => f === 'Description' ? 10000 : f === 'ShortDescription' ? 1000 : f === 'ReleaseNotes' ? 1500
  : /^SearchTerm/.test(f) ? 30 : /^(Feature|DesktopScreenshotCaption)/.test(f) ? 200 : Infinity;

const rows = parseCsv(fs.readFileSync(path.join(here, 'listing-fields.csv'), 'utf8').replace(/^﻿/, ''));
const map = JSON.parse(fs.readFileSync(path.join(here, 'store-languages.json'), 'utf8'));
delete map._comment;
const codes = ['en-us', ...Object.values(map).filter(c => c !== 'en-us')];
const keys = Object.keys(JSON.parse(fs.readFileSync(path.join(here, 'listing', 'en-us.json'), 'utf8')));
const texts = {};
for (const c of codes) {
  const j = JSON.parse(fs.readFileSync(path.join(here, 'listing', c + '.json'), 'utf8'));
  for (const k of keys) {
    if (!String(j[k] ?? '').trim()) throw new Error(`${c}: ${k} is missing`);
    if ([...j[k]].length > limit(k)) throw new Error(`${c}: ${k} is longer than ${limit(k)} characters`);
  }
  texts[c] = j;
}

const shots = ['snagster', 'studio', 'stream', 'ai-apps', 'streaming-settings'];
const folder = path.basename(outDir);
fs.mkdirSync(outDir, { recursive: true });
const defaults = {
  Title: 'Snagster',
  DevStudio: 'SoloSaftis',
  CopyrightTrademarkInformation: '© SoloSaftis. Free and open source under the MIT or Apache-2.0 license.',
  StoreLogo300x300: `${folder}/store-logo-300.png`,
};
shots.forEach((s, i) => {
  const name = `screenshot-${i + 1}-${s}.png`;
  fs.copyFileSync(path.join(root, 'assets', 'screenshots', s + '.png'), path.join(outDir, name));
  defaults[`DesktopScreenshot${i + 1}`] = `${folder}/${name}`;
});
if (!fs.existsSync(path.join(outDir, 'store-logo-300.png'))) {
  console.warn(`Write the logo first: snagster --icon-png ${path.join(outDir, 'store-logo-300.png')} 300`);
}

const out = [[...rows[0].slice(0, 4), ...codes]];
for (const r of rows.slice(1)) {
  if (!r[0]) continue;
  out.push([r[0], r[1], r[2], defaults[r[0]] ?? '', ...codes.map(c => (r[0] in texts[c] ? texts[c][r[0]] : ''))]);
}
fs.writeFileSync(path.join(outDir, 'listings.csv'), '﻿' + out.map(l => l.map(cell).join(',')).join('\r\n') + '\r\n', 'utf8');
console.log(`${codes.length} languages, ${out.length - 1} fields -> ${outDir}`);
