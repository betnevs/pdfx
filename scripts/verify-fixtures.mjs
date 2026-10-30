import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';

// A line-ending conversion breaks PDF xref offsets and stream lengths.
const root = new URL('../fixtures/', import.meta.url);
const manifest = JSON.parse(readFileSync(new URL('manifest.json', root), 'utf8'));
for (const fixture of manifest.fixtures) {
  const bytes = readFileSync(new URL(fixture.path, root));
  const hash = createHash('sha256').update(bytes).digest('hex');
  if (hash !== fixture.sha256 || bytes.length !== fixture.size_bytes) {
    throw new Error(`Fixture changed during checkout: ${fixture.path}`);
  }
}
console.log(`All ${manifest.fixtures.length} PDF fixture hashes and sizes match.`);
