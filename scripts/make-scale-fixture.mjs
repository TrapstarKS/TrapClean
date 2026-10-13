import { mkdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

const root = resolve(process.argv[2]);
const count = Number(process.argv[3] ?? 100000);
if (!Number.isInteger(count) || count < 100000 || count > 1000000) throw new Error('Use 100000–1000000 generated files.');
await mkdir(root); // Refuse an existing directory; never replace user data.
for (let group = 0; group < Math.ceil(count / 1000); group++) {
  const directory = join(root, `group-${String(group).padStart(4, '0')}`);
  await mkdir(directory);
  for (let offset = 0; offset < 1000; offset += 16) {
    await Promise.all(Array.from({ length: Math.min(16, 1000 - offset, count - group * 1000 - offset) }, (_, index) => writeFile(join(directory, `file-${offset + index}.dat`), 'generated fixture data')));
  }
}
console.log(`${count} generated files at ${root}`);
