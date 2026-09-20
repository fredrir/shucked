import * as fs from 'node:fs';
import * as path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const requiredHelpers = ['awk', 'basename', 'cat', 'cut', 'dirname', 'find', 'grep', 'head', 'ls', 'readlink', 'sed', 'sort', 'tail', 'tr', 'uniq', 'wc', 'xargs'];
const checksum = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');

function verifyArchitecture(file, relative, target) {
  if (relative.startsWith('sources/') || target.startsWith('win32-')) { return; }
  const header = Buffer.alloc(64);
  const descriptor = fs.openSync(file, 'r');
  try { fs.readSync(descriptor, header, 0, 64, 0); } finally { fs.closeSync(descriptor); }
  if (header.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) {
    if (!target.startsWith('linux-') && !target.startsWith('alpine-')) { throw new Error('Provider binary OS mismatch: ' + relative); }
    const expected = target.endsWith('-arm64') ? 183 : target.endsWith('-armhf') ? 40 : 62;
    const actual = header[5] === 1 ? header.readUInt16LE(18) : header.readUInt16BE(18);
    if (actual !== expected) { throw new Error('Provider ELF architecture mismatch: ' + relative); }
  } else if (header.readUInt32LE(0) === 0xfeedfacf || header.readUInt32LE(0) === 0xfeedface) {
    if (!target.startsWith('darwin-')) { throw new Error('Provider binary OS mismatch: ' + relative); }
    const expected = target.endsWith('-arm64') ? 0x0100000c : 0x01000007;
    if (header.readUInt32LE(4) !== expected) { throw new Error('Provider Mach-O architecture mismatch: ' + relative); }
  } else if (['bin/bash', 'bin/zsh', 'bin/fish'].includes(relative)) {
    throw new Error('Provider engine is not a target-native binary: ' + relative);
  }
}

export function providerWorkerInputs() {
  const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  return {
    packManifest: checksum(path.join(repo, 'tooling/providers/packs/manifest.json')),
    workers: Object.fromEntries(['bash_worker.bash', 'fish_worker.fish', 'zsh_worker.zsh', 'zsh_supervisor.zsh'].map(name => [name, checksum(path.join(repo, 'crates/shucked-lsp/src/handlers/completion', name))])),
  };
}

export function verifyProviderRuntime(root, target) {
  const manifest = JSON.parse(fs.readFileSync(path.join(root, 'manifest.json'), 'utf8'));
  if (manifest.schemaVersion !== 2 || manifest.target !== target) { throw new Error('Provider runtime schema/target mismatch'); }
  if (manifest.validation?.workers !== 'passed') { throw new Error('Provider workers have not passed on this target'); }
  if (JSON.stringify(manifest.validation.inputs) !== JSON.stringify(providerWorkerInputs())) { throw new Error('Provider tested worker inputs changed'); }
  if (!manifest.sources?.length) { throw new Error('Provider source provenance missing'); }
  if (!requiredHelpers.every(name => manifest.helperNames?.includes(name))) { throw new Error('Private helper suite incomplete'); }
  if (!fs.existsSync(path.join(root, 'sbom.spdx.json'))) { throw new Error('Provider SBOM missing'); }
  const actual = [];
  const realRoot = fs.realpathSync(root);
  const walk = directory => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const file = path.join(directory, entry.name);
      const relative = path.relative(root, file).split(path.sep).join('/');
      const resolved = fs.realpathSync(file);
      if (resolved !== realRoot && !resolved.startsWith(realRoot + path.sep)) { throw new Error('Provider symlink escapes runtime'); }
      if (entry.isDirectory()) { walk(file); }
      else if (fs.statSync(file).isFile() && relative !== 'manifest.json') { verifyArchitecture(file, relative, target); actual.push({ path: relative, sha256: checksum(file), executable: Boolean(fs.statSync(file).mode & 0o111) }); }
    }
  };
  walk(root);
  actual.sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  if (JSON.stringify(actual) !== JSON.stringify(manifest.files)) { throw new Error('Provider file inventory mismatch'); }
  for (const source of manifest.sources ?? []) {
    if (!source.license || !source.archives?.length) { throw new Error('Provider source provenance missing'); }
    for (const archive of source.archives) {
      if (!actual.some(file => file.path === archive.path && file.sha256 === archive.sha256)) { throw new Error('Corresponding provider source archive missing'); }
    }
  }
  if (!target.startsWith('win32-')) {
    for (const [directory, names] of [['bin', ['bash', 'zsh', 'fish']], ['helpers/bin', requiredHelpers]]) {
      for (const name of names) {
        const file = path.join(root, directory, name);
        if (!fs.existsSync(file) || !(fs.statSync(file).mode & 0o111)) { throw new Error('Provider executable permission missing'); }
      }
    }
  }
  return manifest;
}
