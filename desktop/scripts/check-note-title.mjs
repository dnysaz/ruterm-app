import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { noteTitle } from '../src/lib/note-title.ts';

const here = path.dirname(fileURLToPath(import.meta.url));
const fixtures = JSON.parse(
  readFileSync(path.join(here, '../src/lib/note-title-fixtures.json'), 'utf8'),
);

let failed = 0;
for (const { body, title } of fixtures) {
  const actual = noteTitle(body);
  if (actual !== title) {
    failed += 1;
    console.error(`FAIL ${JSON.stringify(body)}\n  expected: ${title}\n  actual:   ${actual}`);
  }
}
if (failed) process.exit(1);
console.log(`note-title fixtures: ${fixtures.length}/${fixtures.length} passed`);
