import test from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { createHash } from 'node:crypto';
import { verifyProviderRuntime, providerWorkerInputs } from '../provider-artifacts.mjs';

function fixture(t, target = 'linux-arm64') {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'shucked-runtime-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const contents = { 'sbom.spdx.json': '{}', 'source.tar': 'source fixture' };
  const helpers = ['awk', 'basename', 'cat', 'cut', 'dirname', 'find', 'grep', 'head', 'ls', 'readlink', 'sed', 'sort', 'tail', 'tr', 'uniq', 'wc', 'xargs'];
  const windows = target.startsWith('win32-');
  const engine = Buffer.alloc(windows ? 128 : 64);
  engine.set([0x7f, 0x45, 0x4c, 0x46, 2, 1]);
  engine.writeUInt16LE(183, 18);
  if (windows) { engine.fill(0); engine.write('MZ'); engine.writeUInt32LE(64, 60); engine.writeUInt32LE(0x4550, 64); engine.writeUInt16LE(0x8664, 68); }
  for (const name of ['bash', 'zsh', 'fish']) { contents[(windows ? 'msys/usr/bin/' + name + '.exe' : 'bin/' + name)] = engine; }
  for (const name of helpers) { contents['helpers/bin/' + name] = 'helper'; }
  const files = Object.entries(contents).map(([name, content]) => {
    fs.mkdirSync(path.dirname(path.join(root, name)), { recursive: true });
    fs.writeFileSync(path.join(root, name), content);
    if (name.includes('bin/')) { fs.chmodSync(path.join(root, name), 0o755); }
    return { path: name, sha256: createHash('sha256').update(content).digest('hex'), executable: name.includes('bin/') };
  });
  const manifest = {
    schemaVersion: 2, target, architecture: windows ? 'x86_64' : 'aarch64', minimumHost: windows ? { binaryArchitecture: 'x64', emulation: target.endsWith('arm64') ? 'Windows x64' : null, windowsBuild: 22000 } : {}, validation: { workers: 'passed', inputs: providerWorkerInputs() },
    helperNames: ['awk', 'basename', 'cat', 'cut', 'dirname', 'find', 'grep', 'head', 'ls', 'readlink', 'sed', 'sort', 'tail', 'tr', 'uniq', 'wc', 'xargs'],
    sources: [{ license: 'MIT', archives: [{ path: 'source.tar', sha256: files[1].sha256 }] }], files,
  };
  manifest.files.sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  const write = () => fs.writeFileSync(path.join(root, 'manifest.json'), JSON.stringify(manifest));
  write();
  return { root, manifest, write };
}

test('runtime packaging requires matching target and passed worker tests', t => {
  const { root, manifest, write } = fixture(t);
  verifyProviderRuntime(root, 'linux-arm64');
  assert.throws(() => verifyProviderRuntime(root, 'alpine-arm64'), /target mismatch/);
  manifest.validation.workers = 'not-run';
  write();
  assert.throws(() => verifyProviderRuntime(root, 'linux-arm64'), /not passed/);
});

test('runtime packaging rejects extra files and missing corresponding sources', t => {
  const { root, manifest, write } = fixture(t);
  fs.writeFileSync(path.join(root, 'untracked'), 'unexpected executable');
  assert.throws(() => verifyProviderRuntime(root, 'linux-arm64'), /inventory mismatch/);
  fs.unlinkSync(path.join(root, 'untracked'));
  manifest.sources[0].archives[0].path = 'missing.tar';
  write();
  assert.throws(() => verifyProviderRuntime(root, 'linux-arm64'), /source archive missing/);
});

test('runtime packaging rejects workers changed after smoke validation', t => {
  const { root, manifest, write } = fixture(t);
  manifest.validation.inputs.workers['bash_worker.bash'] = 'old-worker';
  write();
  assert.throws(() => verifyProviderRuntime(root, 'linux-arm64'), /worker inputs changed/);
});

test('runtime packaging rejects executable bits removed after validation', t => {
  const { root } = fixture(t);
  fs.chmodSync(path.join(root, 'helpers/bin/grep'), 0o644);
  assert.throws(() => verifyProviderRuntime(root, 'linux-arm64'), /inventory mismatch/);
});

test('runtime packaging rejects binaries of another CPU even with matching inventory', t => {
  const { root, manifest, write } = fixture(t);
  const file = path.join(root, 'bin/fish');
  const binary = fs.readFileSync(file);
  binary.writeUInt16LE(62, 18);
  fs.writeFileSync(file, binary);
  manifest.files.find(item => item.path === 'bin/fish').sha256 = createHash('sha256').update(binary).digest('hex');
  write();
  assert.throws(() => verifyProviderRuntime(root, 'linux-arm64'), /architecture mismatch/);
});

for (const target of ['win32-x64', 'win32-arm64']) {
  test(`runtime packaging verifies PE architecture for ${target}`, t => {
    const { root, manifest, write } = fixture(t, target);
    verifyProviderRuntime(root, target);
    const relative = 'msys/usr/bin/fish.exe';
    const file = path.join(root, relative);
    const binary = fs.readFileSync(file);
    binary.writeUInt16LE(0xaa64, 68);
    fs.writeFileSync(file, binary);
    manifest.files.find(item => item.path === relative).sha256 = createHash('sha256').update(binary).digest('hex');
    write();
    assert.throws(() => verifyProviderRuntime(root, target), /PE architecture mismatch/);
  });
}

test('Windows ARM64 packaging requires declared x64 emulation', t => {
  const { root, manifest, write } = fixture(t, 'win32-arm64');
  delete manifest.minimumHost.emulation;
  write();
  assert.throws(() => verifyProviderRuntime(root, 'win32-arm64'), /emulation declaration/);
});
