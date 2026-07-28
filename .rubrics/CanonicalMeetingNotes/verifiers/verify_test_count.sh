#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
node <<'NODE'
const fs = require('fs');
const path = require('path');

function filesUnder(root, suffix) {
  if (!fs.existsSync(root)) return [];
  const out = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const full = path.join(root, entry.name);
    if (entry.isDirectory()) out.push(...filesUnder(full, suffix));
    else if (entry.name.endsWith(suffix)) out.push(full);
  }
  return out;
}

let count = 0;
for (const file of [...filesUnder('crates', '.rs'), ...filesUnder('xtask', '.rs')]) {
  count += (fs.readFileSync(file, 'utf8').match(/#\[(?:wasm_bindgen_)?test\]/g) || []).length;
}
for (const file of [...filesUnder('tests', '.mjs'), ...filesUnder('.evals', '.mjs')]) {
  count += (fs.readFileSync(file, 'utf8').match(/(?:^|\n)\s*test\s*\(/g) || []).length;
}
if (count < 428) throw new Error(`test count ${count} is below baseline 428`);
console.log(count);
NODE
