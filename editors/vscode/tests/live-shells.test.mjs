import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
const execute = promisify(execFile);
for (const shell of ['bash', 'zsh', 'fish']) {
  test(`${shell} live completion uses current functions and variables without evaluating editor words`, async t => {
    if (process.platform === 'win32') { t.skip('Unix signal transport'); return; }
    try { await execute(shell, ['--version']); }
    catch (error) { if (error.code === 'ENOENT') { t.skip(`${shell} unavailable`); return; } throw error; }
    const result = await execute('python3', [fileURLToPath(new URL('./live-shell-probe.py', import.meta.url)), shell], { timeout: 15000, maxBuffer: 65536 });
    assert.equal(JSON.parse(result.stdout).passed, true);
  });
}

for (const shell of ['bash', 'zsh']) {
  test(`${shell} watchdog terminates hanging callbacks and children without ps or editor cleanup`, async t => {
    if (process.platform === 'win32') { t.skip('Unix signal transport'); return; }
    try { await execute(shell, ['--version']); }
    catch (error) { if (error.code === 'ENOENT') { t.skip(`${shell} unavailable`); return; } throw error; }
    const result = await execute('python3', [fileURLToPath(new URL('./live-shell-probe.py', import.meta.url)), shell, 'timeout'], { timeout: 15000, maxBuffer: 65536 });
    assert.equal(JSON.parse(result.stdout).watchdog, true);
  });
}

for (const shell of ['bash', 'zsh', 'fish']) {
  test(`${shell} preserves occupied user signal handlers and declines live queries`, async t => {
    if (process.platform === 'win32') { t.skip('Unix signal transport'); return; }
    try { await execute(shell, ['--version']); }
    catch (error) { if (error.code === 'ENOENT') { t.skip(`${shell} unavailable`); return; } throw error; }
    const result = await execute('python3', [fileURLToPath(new URL('./live-shell-probe.py', import.meta.url)), shell, 'occupied'], { timeout: 15000, maxBuffer: 65536 });
    assert.equal(JSON.parse(result.stdout).preservedTrap, true);
  });
}

for (const shell of ['bash', 'zsh']) {
  test(`${shell} terminates background children even when the callback returns candidates`, async t => {
    if (process.platform === 'win32') { t.skip('Unix signal transport'); return; }
    try { await execute(shell, ['--version']); }
    catch (error) { if (error.code === 'ENOENT') { t.skip(`${shell} unavailable`); return; } throw error; }
    const result = await execute('python3', [fileURLToPath(new URL('./live-shell-probe.py', import.meta.url)), shell, 'background'], { timeout: 15000, maxBuffer: 65536 });
    assert.equal(JSON.parse(result.stdout).watchdog, true);
  });
}

test('Bash live completers distinguish raw filename spaces from shell-word quoting', async t => {
  if (process.platform === 'win32') { t.skip('Unix signal transport'); return; }
  const result = await execute('python3', [fileURLToPath(new URL('./live-shell-probe.py', import.meta.url)), 'bash', 'quoting'], { timeout: 15000, maxBuffer: 65536 });
  assert.equal(JSON.parse(result.stdout).passed, true);
});
