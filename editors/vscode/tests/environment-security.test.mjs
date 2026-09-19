import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";
import { createRequire } from "node:module";
import { runInNewContext } from "node:vm";
import { fileURLToPath } from "node:url";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
const require = createRequire(import.meta.url);
async function compiled(filename) {
  return (await build({ entryPoints: [fileURLToPath(new URL(`../src/${filename}`, import.meta.url))], bundle: true, platform: "node", format: "cjs", external: ["vscode"], write: false })).outputFiles[0].text;
}
function moduleFrom(code) {
  const module = { exports: {} };
  runInNewContext(code, { module, exports: module.exports, process, Buffer, require: id => id === "vscode" ? {} : require(id) });
  return module.exports;
}
const historyCode = await compiled("history.ts");
const { readHistoryFile } = moduleFrom(historyCode);
const { launchDirectoryLabel } = moduleFrom(await compiled("environment.ts"));
test("startup status never presents post-startup terminal cwd as entry context", () => {
  assert.match(launchDirectoryLabel(true, { sessionId: "terminal", cwd: "/configured" }, "/after-startup"), /unknown at startup/);
  assert.equal(launchDirectoryLabel(false, { sessionId: "terminal" }, "/live"), "/live");
  assert.equal(launchDirectoryLabel(false, { cwd: "/configured" }), "/configured");
});
test("history reader returns regular file data and rejects directories", async () => {
  const root = mkdtempSync(join(tmpdir(), "shucked-history-"));
  try {
    const file = join(root, "history");
    writeFileSync(file, "git status\n");
    assert.equal(await readHistoryFile(file), "git status\n");
    assert.equal(await readHistoryFile(root), undefined);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
test("history named pipes cannot block an extension I/O worker", { skip: process.platform === "win32" }, () => {
  const root = mkdtempSync(join(tmpdir(), "shucked-history-fifo-"));
  try {
    const fifo = join(root, "history");
    execFileSync("mkfifo", [fifo]);
    const script = `const vm=require('node:vm'); const loaded={exports:{}}; vm.runInNewContext(${JSON.stringify(historyCode)}, {module:loaded,exports:loaded.exports,process,Buffer,require:id=>id==='vscode'?{}:require(id)}); loaded.exports.readHistoryFile(process.argv[1]).then(value=>process.stdout.write(String(value)));`;
    assert.equal(execFileSync(process.execPath, ["-e", script, fifo], { timeout: 1500, encoding: "utf8" }), "undefined");
  } finally { rmSync(root, { recursive: true, force: true }); }
});
