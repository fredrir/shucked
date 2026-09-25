import assert from "node:assert/strict";
import { test } from "node:test";
import { load } from "./support/load.mjs";

const { validateSessionMessage, validateDropNotice, dropWarning, bashLoginInit, zshLoginFiles, MAX_FRAME, MAX_NAMES } = await load("terminal.ts");
const valid = {
  id: "a".repeat(32), token: "b".repeat(64), generation: 1, pid: 123, shell: "zsh", cwd: process.cwd(),
  path: ["", "/usr/bin"], aliases: { ls: ["eza", "--icons"] }, functions: ["greet"], options: { aliases: "on" },
  private: false, ignore: [], connected: true,
};

test("well-formed shell metadata is accepted", () => {
  assert.equal(validateSessionMessage(valid), true);
  assert.equal(validateSessionMessage({ ...valid, historyFile: "", liveCompletion: true, liveSignal: "SIGUSR2", acceptedHistoryHash: "c".repeat(64) }), true);
});

test("shell metadata rejects malformed and oversized identities", () => {
  for (const patch of [
    { generation: -1 }, { generation: Infinity }, { generation: 0 }, { pid: 0 },
    { id: "A".repeat(32) }, { id: "a".repeat(31) }, { token: "no" },
    { shell: "powershell" }, { cwd: "relative" }, { cwd: "/tmp/\0" },
    { path: [null] }, { functions: Array(MAX_NAMES + 1).fill("f") }, { ignore: Array(33).fill("x") },
    { aliases: { ls: "eza" } }, { aliases: { "bad name": ["x"] } }, { aliases: [] },
    { options: { "bad key": "x" } }, { options: { key: "x".repeat(128) } },
    { acceptedHistoryHash: "short" }, { historyFile: "relative/history" }, { historyFile: 42 },
    { liveCompletion: "yes" }, { liveSignal: "SIGKILL" }, { private: "no" }, { connected: false },
  ]) {
    assert.equal(validateSessionMessage({ ...valid, ...patch }), false, JSON.stringify(patch).slice(0, 80));
  }
  for (const value of [null, undefined, "text", 42, []]) {
    assert.equal(validateSessionMessage(value), false);
  }
});

test("large but bounded inventories are accepted up to the server's truncation limit", () => {
  const functions = Array.from({ length: MAX_NAMES }, (_, index) => `f${index}`);
  assert.equal(validateSessionMessage({ ...valid, functions }), true);
  assert.equal(MAX_NAMES, 50000);
  assert.equal(MAX_FRAME, 1024 * 1024, "one frame matches the hook's payload cap");
});

test("hook drop notices carry the session identity and a known reason", () => {
  const notice = { kind: "hookDropped", id: valid.id, token: valid.token, generation: 2, pid: 123, shell: "zsh", reason: "size" };
  assert.equal(validateDropNotice(notice), true);
  assert.equal(validateDropNotice({ ...notice, reason: "deadline" }), true);
  for (const patch of [{ reason: "mystery" }, { kind: "liveCompletion" }, { token: "x" }, { id: "short" }, { generation: 0 }, { pid: -1 }, { shell: "csh" }]) {
    assert.equal(validateDropNotice({ ...notice, ...patch }), false, JSON.stringify(patch));
  }
  assert.equal(validateDropNotice(valid), false, "metadata is not a drop notice");
});

test("the one-time drop warning explains size, deadline and authentication failures", () => {
  assert.match(dropWarning("zsh", "size"), /zsh terminal .* 1024 KiB/);
  assert.match(dropWarning("bash", "deadline"), /did not finish within the deadline/);
  assert.match(dropWarning("fish", "authentication"), /failed authentication checks/);
});

test("the bash rc file performs the login startup order before the hook", () => {
  const init = bashLoginInit("/ext/shell-integration/bash.sh");
  const lines = init.trimEnd().split("\n");
  assert.equal(lines[0], "[[ -f /etc/profile ]] && source /etc/profile");
  assert.ok(lines.some(line => line.includes("~/.bash_profile ~/.bash_login ~/.profile")), "profile files are tried in bash's login order");
  assert.ok(lines.some(line => line.includes("break")), "only the first profile file is read, as a login shell does");
  assert.ok(lines.some(line => /__shucked_profile_read = 0 .* ~\/\.bashrc/.test(line)), "~/.bashrc still applies when no profile exists");
  assert.equal(lines.at(-1), "source '/ext/shell-integration/bash.sh'", "the hook loads after the user's startup files");
  assert.ok(init.endsWith("\n"));
  assert.match(bashLoginInit("/it's/bash.sh"), /source '\/it'\\''s\/bash\.sh'/, "hook paths are shell-quoted");
});

test("the private ZDOTDIR chains every zsh login file and sources the hook after .zshrc", () => {
  const files = zshLoginFiles("/home/me", "/tmp/session", "/ext/zsh.zsh");
  assert.deepEqual(Object.keys(files), [".zshenv", ".zprofile", ".zshrc"]);
  assert.match(files[".zshenv"], /^ZDOTDIR='\/home\/me'\n\[\[ -r \$ZDOTDIR\/\.zshenv \]\] && source \$ZDOTDIR\/\.zshenv\n__shucked_original_zdotdir=\$ZDOTDIR\nZDOTDIR='\/tmp\/session'\n$/);
  assert.match(files[".zprofile"], /source \$ZDOTDIR\/\.zprofile\nZDOTDIR='\/tmp\/session'\n$/, ".zprofile hands back to the private directory so .zshrc chains next");
  assert.match(files[".zshrc"], /source \$ZDOTDIR\/\.zshrc\nsource '\/ext\/zsh\.zsh'\n$/);
  assert.ok(!files[".zshrc"].includes("ZDOTDIR='/tmp/session'"), ".zlogin is then read from the user's ZDOTDIR");
});
