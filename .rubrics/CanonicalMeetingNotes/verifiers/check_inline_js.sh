#!/usr/bin/env bash
set -euo pipefail
cd /Users/mike/dev/silent-notetaker
node <<'NODE'
const fs = require('fs');
const vm = require('vm');
const html = fs.readFileSync('index.html', 'utf8');
const scripts = [...html.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/gi)];
if (scripts.length < 3) throw new Error(`expected at least 3 inline scripts, got ${scripts.length}`);
for (let index = 1; index < scripts.length; index += 1) {
  new vm.Script(scripts[index][1], { filename: `index-inline-${index}.js` });
}
console.log(scripts.length - 1);
NODE
