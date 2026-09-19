import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const extension = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repository = resolve(extension, '../..');
const server = process.env.SHUCKED_TEST_SERVER ?? join(repository, 'target/debug/shucked');
const vsix = process.env.SHUCKED_TEST_VSIX ? resolve(process.env.SHUCKED_TEST_VSIX) : undefined;
await access(vsix ?? server);
const root = await mkdtemp(join(tmpdir(), 'shucked-extension-host-'));
const workspace = join(root, 'workspace'), home = join(root, 'home'), user = join(root, 'user'), extensions = join(root, 'extensions');
await Promise.all([workspace, home, join(user, 'User'), extensions, join(home, '.config/fish')].map(directory => mkdir(directory, { recursive: true, mode: 0o700 })));
await writeFile(join(user, 'User/settings.json'), JSON.stringify({ ...(vsix ? {} : { 'shucked.server.path': server }), 'shucked.lint.showSyntaxErrors': true, 'shucked.trace.server': 'verbose', 'security.workspace.trust.enabled': false, 'telemetry.telemetryLevel': 'off', 'update.mode': 'none', 'extensions.autoUpdate': false, 'workbench.startupEditor': 'none', 'window.restoreWindows': 'none' }));
await writeFile(join(workspace, 'smoke.zsh'), '#!/bin/zsh\nshucked_smoke_function() { printf ok; }\nshucked_missing_smoke\nshucked_smoke_f\n');
await writeFile(join(workspace, 'smoke.fish'), 'function fish_fixture\n echo hello\nend\nfish_fi\n');
await Promise.all(['.bashrc', '.zshrc'].map(name => writeFile(join(home, name), "alias shucked_smoke_alias='printf'\n")));
await writeFile(join(home, '.config/fish/config.fish'), "alias shucked_smoke_alias='printf'\n");
const resultPath = join(root, 'result.json');
const launchEnvironment = { ...process.env, HOME: home, ZDOTDIR: home, XDG_CONFIG_HOME: join(home, '.config'), SHUCKED_EXTENSION_TEST_RESULT: resultPath, SHUCKED_EXTENSION_TEST_PACKAGED: vsix ? '1' : '0' };
let developmentExtension = extension;
if (vsix) {
  // A tiny runner hosts tests while fredrir.shucked comes from the installed package.
  developmentExtension = join(root, 'runner');
  await mkdir(developmentExtension, { mode: 0o700 });
  await writeFile(join(developmentExtension, 'package.json'), JSON.stringify({ name: 'shucked-acceptance-runner', publisher: 'shucked-tests', version: '0.0.1', engines: { vscode: '^1.138.0' }, main: './index.cjs', activationEvents: ['*'] }));
  await writeFile(join(developmentExtension, 'index.cjs'), 'exports.activate = () => undefined;\n');
  execFileSync(process.env.SHUCKED_CODE_COMMAND ?? 'code', ['--user-data-dir', user, '--extensions-dir', extensions, '--use-inmemory-secretstorage', '--install-extension', vsix, '--force'], { env: launchEnvironment, stdio: 'pipe', timeout: 60000 });
}
console.log(`Extension Development Host evidence: ${root}`);
// Keep secrets in memory so an isolated macOS HOME never asks to initialize a keychain.
const child = spawn(process.env.SHUCKED_CODE_COMMAND ?? 'code', ['--new-window', '--log', 'trace', '--wait', '--user-data-dir', user, '--extensions-dir', extensions, ...(vsix ? [] : ['--disable-extensions']), '--use-inmemory-secretstorage', '--skip-welcome', '--skip-release-notes', '--disable-workspace-trust', `--extensionDevelopmentPath=${developmentExtension}`, `--extensionTestsPath=${join(extension, 'tests/extension-host.cjs')}`, workspace], { env: launchEnvironment, stdio: ['ignore', 'pipe', 'pipe'] });
let output = '';
for (const stream of [child.stdout, child.stderr]) {stream.on('data', chunk => { output = (output + chunk.toString()).slice(-16000); });}
// Code's CLI can detach the desktop process; stopping only the wrapper leaves it alive.
// Scope cleanup to the freshly created profile path, never another Code window.
function stopHost() {
  if (process.platform !== 'win32') {
    const rows = execFileSync('ps', ['-axo', 'pid=,args='], { encoding: 'utf8' }).split('\n');
    for (const row of rows) {
      const match = row.trim().match(/^(\d+)\s+(.*)$/);
      if (match && match[2].includes(user)) {
        try { process.kill(Number(match[1]), 'SIGKILL'); } catch { /* The process may already have exited. */ }
      }
    }
  }
  child.kill('SIGTERM');
}
const watchdog = setTimeout(stopHost, 150000);
for (const signal of ['SIGINT', 'SIGTERM']) {process.once(signal, () => { stopHost(); process.exit(1); });}
const exit = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', code => resolve(code)); });
clearTimeout(watchdog);
stopHost();
try { const report = JSON.parse(await readFile(resultPath, 'utf8')); console.log(JSON.stringify(report, null, 2)); process.exitCode = report.passed && exit === 0 ? 0 : 1; }
catch { console.error(output); console.error(`Extension host exited ${exit} without a test report; logs: ${user}/logs`); process.exitCode = 1; }
