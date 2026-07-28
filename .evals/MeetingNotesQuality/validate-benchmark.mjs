import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.dirname(fileURLToPath(import.meta.url));
const readJson = (relative) => JSON.parse(fs.readFileSync(path.join(root, relative), 'utf8'));
const readJsonl = (relative) => fs.readFileSync(path.join(root, relative), 'utf8')
  .split(/\r?\n/)
  .filter(Boolean)
  .map((line) => JSON.parse(line));

const benchmark = readJson('benchmark.json');
const items = readJsonl(benchmark.items);
const subjects = readJsonl(benchmark.subjects);

if (benchmark.id !== 'MeetingNotesQuality-v1') throw new Error('unexpected benchmark id');
if (!/No IRT/i.test(benchmark.claimScope)) throw new Error('claim scope must explicitly reject IRT claims');
if (items.length < benchmark.minimumItems) throw new Error('item bank below minimum');
if (subjects.length < 1) throw new Error('at least one frozen subject is required');
if (new Set(items.map((item) => item.id)).size !== items.length) throw new Error('duplicate item ids');

for (const item of items) {
  if (item.split !== 'public') throw new Error(`${item.id}: unexpected split`);
  const fixturePath = path.join(root, item.fixture);
  const bytes = fs.readFileSync(fixturePath);
  const hash = crypto.createHash('sha256').update(bytes).digest('hex');
  if (item.fixtureSha256 !== hash) throw new Error(`${item.id}: fixture hash mismatch`);
  const fixture = JSON.parse(bytes);
  if (fixture.id !== item.id) throw new Error(`${item.id}: fixture id mismatch`);
  if (!Array.isArray(fixture.transcriptSegments) || fixture.transcriptSegments.length < 3) {
    throw new Error(`${item.id}: transcript is not substantive`);
  }
  if (!Array.isArray(fixture.expectedConcepts) || fixture.expectedConcepts.length < 4) {
    throw new Error(`${item.id}: too few scored concepts`);
  }
  if (!Array.isArray(fixture.forbiddenClaims) || fixture.forbiddenClaims.length < 3) {
    throw new Error(`${item.id}: insufficient non-invention checks`);
  }
}

for (const subject of subjects) {
  if (subject.status !== 'frozen') throw new Error(`${subject.id}: subject is not frozen`);
  if (subject.backend !== 'transformers.js') throw new Error(`${subject.id}: backend is not real browser inference`);
  if (!/^onnx-community\/Qwen3-/.test(subject.model)) throw new Error(`${subject.id}: model is not Qwen3`);
}

console.log(`validated ${items.length} items and ${subjects.length} frozen subject`);
