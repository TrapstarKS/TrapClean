import { mkdir, open, writeFile, link, symlink } from 'node:fs/promises';
import { resolve, join } from 'node:path';

const destination = resolve(process.argv[2] ?? '/tmp/TrapClean Demo');
await mkdir(destination); // Refuse existing folders: this script never replaces user data.
const dirs = ['Projects/atlas/src', 'Projects/atlas/target/debug/deps', 'Projects/site/node_modules/example', 'Projects/site/dist', '.npm/_cacache/content-v2', '.npm/_cacache/index-v5', 'Media', 'Documents', 'Downloads'];
for (const dir of dirs) await mkdir(join(destination, dir), { recursive: true });
const files = {
  'Projects/atlas/Cargo.toml': "[package]\nname='atlas'\nversion='0.1.0'\nedition='2021'\n",
  'Projects/atlas/src/lib.rs': 'pub fn atlas() {}\n',
  'Projects/atlas/target/.rustc_info.json': '{}',
  'Projects/site/package.json': '{"name":"site","private":true}',
  'Projects/site/package-lock.json': '{"lockfileVersion":3}',
  'Projects/site/node_modules/.package-lock.json': '{}',
  'Projects/site/node_modules/example/index.js': 'export const example = 1;\n',
  'Projects/site/dist/index.html': '<h1>Generated demo output</h1>',
  'Documents/notes.txt': 'Generated fixture. No private data.\n',
};
for (const [path, content] of Object.entries(files)) await writeFile(join(destination, path), content);
for (const [path, length] of [['Media/recording.mov', 1_600_000_000], ['Media/photos.zip', 400_000_000], ['Projects/atlas/target/debug/deps/atlas.rlib', 900_000_000], ['.npm/_cacache/content-v2/demo-cache', 220_000_000]]) {
  const file = await open(join(destination, path), 'wx');
  await file.write(Buffer.alloc(4096, 0x64));
  await file.truncate(length);
  await file.close();
}
const duplicate = Buffer.alloc(1_048_576, 0x42);
await writeFile(join(destination, 'Downloads/reference-copy.txt'), duplicate);
await writeFile(join(destination, 'Documents/reference.txt'), duplicate);
await link(join(destination, 'Documents/notes.txt'), join(destination, 'Documents/notes-hardlink.txt'));
if (process.platform !== 'win32') await symlink('../Media', join(destination, 'Downloads/media-link'));
console.log(destination);
