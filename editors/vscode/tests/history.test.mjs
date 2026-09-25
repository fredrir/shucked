import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { bundle, evaluate } from "./support/load.mjs";

const compiled = await bundle("history.ts");
const { HistoryIndex, parseHistory, acceptedSessionCommand, readHistoryFile } = evaluate(compiled);
const sha = text => createHash("sha256").update(text).digest("hex");

test("history index isolates targets and never proposes private, multiline, or oversized entries", () => {
  const index = new HistoryIndex();
  for (const command of ["brew install fish", " secret token", "brew\nsecret", "b".repeat(9000)]) { index.record("hostA", command); }
  assert.equal(index.suggest("hostA", "brew"), "brew install fish");
  assert.equal(index.suggest("hostB", "brew"), undefined);
  assert.equal(index.suggest("hostA", " secret"), undefined);
  index.clear("hostA");
  assert.equal(index.suggest("hostA", "brew"), undefined);
});

test("the most recent matching command wins and repeats move to the end", () => {
  const index = new HistoryIndex();
  for (const command of ["git status", "git stash", "git status"]) { index.record("host", command); }
  assert.equal(index.suggest("host", "git st"), "git status");
  index.record("host", "git stash");
  assert.equal(index.suggest("host", "git st"), "git stash");
});

test("suggestions need a two-character prefix and never repeat the prefix itself", () => {
  const index = new HistoryIndex();
  index.record("host", "ls");
  index.record("host", "ls -la");
  assert.equal(index.suggest("host", "l"), undefined);
  assert.equal(index.suggest("host", "ls -la"), undefined);
  assert.equal(index.suggest("host", "ls"), "ls -la");
});

test("each context keeps at most a thousand commands and only 32 contexts are kept", () => {
  const index = new HistoryIndex();
  for (let number = 0; number < 1005; number++) { index.record("host", `echo ${number}`); }
  assert.equal(index.suggest("host", "echo 0"), undefined, "the oldest commands are dropped");
  assert.equal(index.suggest("host", "echo 100"), "echo 1004");
  for (let context = 0; context < 33; context++) { index.record(`context-${context}`, "echo kept"); }
  assert.equal(index.suggest("context-0", "echo"), undefined, "the oldest context is dropped");
  assert.equal(index.suggest("context-32", "echo"), "echo kept");
  index.clear();
  assert.equal(index.suggest("context-32", "echo"), undefined);
});

test("shell history formats are decoded as data and multiline commands omitted", () => {
  assert.deepEqual(Array.from(parseHistory(": 123:0;brew install fish\n: 124:0;echo a\\\nsecret", "zsh")), ["brew install fish"]);
  assert.deepEqual(Array.from(parseHistory("#1234\ngit status\n", "bash")), ["git status"]);
  assert.deepEqual(Array.from(parseHistory("- cmd: brew install fish\n  when: 42\n- cmd: echo\\nsecret\n", "fish")), ["brew install fish"]);
});

test("an escaped trailing backslash is not a continuation", () => {
  assert.deepEqual(Array.from(parseHistory("echo \\\\\nls\n", "bash")), ["echo \\\\", "ls"]);
  assert.deepEqual(Array.from(parseHistory("- cmd: printf \\\\\\\\\n", "fish")), ["printf \\\\"]);
});

test("only the last thousand history entries are kept", () => {
  const text = Array.from({ length: 1200 }, (_, number) => `echo ${number}`).join("\n");
  const entries = Array.from(parseHistory(text, "bash"));
  assert.equal(entries.length, 1000);
  assert.equal(entries[0], "echo 200");
});

test("session history requires acceptance at a fresh nonprivate prompt", () => {
  const text = "brew install fish";
  const metadata = { private: false, ignore: [], acceptedHistoryHash: sha(text) };
  assert.equal(acceptedSessionCommand(text, metadata), true);
  assert.equal(acceptedSessionCommand(text, { ...metadata, private: true }), false);
  assert.equal(acceptedSessionCommand("brew install SECRET", metadata), false);
  assert.equal(acceptedSessionCommand(text, { ...metadata, acceptedHistoryHash: undefined }), false);
  assert.equal(acceptedSessionCommand(text, { ...metadata, ignore: ["unsupported-history-filter"] }), false);
  assert.equal(acceptedSessionCommand(text, { ...metadata, ignore: ["leading-space"] }), true);
  assert.equal(acceptedSessionCommand(` ${text}`, metadata), false, "a leading space keeps a command private");
});

test("history reader returns regular file data and rejects directories", async (t) => {
  const root = mkdtempSync(join(tmpdir(), "shucked-history-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const file = join(root, "history");
  writeFileSync(file, "git status\n");
  assert.equal(await readHistoryFile(file), "git status\n");
  assert.equal(await readHistoryFile(root), undefined);
});

test("large history files are read from the end, starting at a line boundary", async (t) => {
  const root = mkdtempSync(join(tmpdir(), "shucked-history-large-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const file = join(root, "history");
  const line = `${"x".repeat(99)}\n`;
  writeFileSync(file, `${line.repeat(22_000)}echo newest\n`);
  const text = await readHistoryFile(file);
  assert.ok(text.length <= 2 * 1024 * 1024);
  assert.ok(text.startsWith("x"), "a partial first line is dropped");
  assert.ok(text.endsWith("echo newest\n"));
});

test("history named pipes cannot block an extension I/O worker", { skip: process.platform === "win32" }, (t) => {
  const root = mkdtempSync(join(tmpdir(), "shucked-history-fifo-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const fifo = join(root, "history");
  execFileSync("mkfifo", [fifo]);
  // A separate process, so a blocking read fails the test by timeout instead of hanging it.
  const script = `const vm=require('node:vm'); const loaded={exports:{}}; vm.runInNewContext(${JSON.stringify(compiled.code)}, {module:loaded,exports:loaded.exports,process,Buffer,require:id=>id==='vscode'?{}:require(id)}); loaded.exports.readHistoryFile(process.argv[1]).then(value=>process.stdout.write(String(value)));`;
  assert.equal(execFileSync(process.execPath, ["-e", script, fifo], { timeout: 1500, encoding: "utf8" }), "undefined");
});
