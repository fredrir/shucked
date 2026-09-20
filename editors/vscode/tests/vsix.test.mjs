import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";
import { hostTarget } from "../platform.mjs";

const run = promisify(execFile);

function fixture(t, command) {
  const root = mkdtempSync(join(tmpdir(), "shucked-vsix-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const name of ["vsix.mjs", "platform.mjs"]) {
    copyFileSync(new URL(`../${name}`, import.meta.url), join(root, name));
  }
  const vsce = join(root, "node_modules", "@vscode", "vsce");
  mkdirSync(vsce, { recursive: true });
  writeFileSync(join(vsce, "vsce"), command);
  return (args = []) => run(process.execPath, [join(root, "vsix.mjs"), ...args], { timeout: 20_000 });
}

test("slow packaging reports progress and waits for the artifact to finish", async (t) => {
  const packageVsix = fixture(t, `
    console.log(JSON.stringify(process.argv.slice(2)));
    setTimeout(() => console.log("Packaged successfully"), 10_500);
  `);
  const { stdout, stderr } = await packageVsix(["package", "--out", "extension.vsix"]);
  assert.deepEqual(JSON.parse(stdout.split("\n")[0]), [
    "package", "--no-dependencies", "--target", hostTarget(), "--out", "extension.vsix",
  ]);
  assert.match(stdout, /Packaged successfully/);
  assert.match(stderr, /VSIX package still running \(\d+s elapsed\)/);
});

test("packaging failures retain their exit status and diagnostic", async (t) => {
  const packageVsix = fixture(t, 'console.error("Invalid package"); process.exitCode = 7;');
  await assert.rejects(packageVsix(), (error) => {
    assert.equal(error.code, 7);
    assert.match(error.stderr, /Invalid package/);
    return true;
  });
});

test("target overrides are rejected before invoking vsce", async (t) => {
  const packageVsix = fixture(t, 'console.log("vsce invoked");');
  for (const args of [["--target", "linux-x64"], ["--target=linux-x64"]]) {
    await assert.rejects(packageVsix(["package", ...args]), (error) => {
      assert.match(error.stderr, /--target overrides are disabled/);
      assert.doesNotMatch(error.stdout, /vsce invoked/);
      return true;
    });
  }
});
