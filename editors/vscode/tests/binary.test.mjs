import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { test } from "node:test";
import { bundle, evaluate } from "./support/load.mjs";
import { configuration, outputChannel } from "./support/fake-vscode.mjs";

const compiled = await bundle("binary.ts");

function resolver(environment = process, { values = {}, global = {}, trusted = true } = {}) {
  const vscode = { workspace: { isTrusted: trusted, getConfiguration: () => configuration({ values, global }) } };
  return evaluate(compiled, { vscode, globals: { process: environment } });
}

function temporary(t, prefix) {
  const root = mkdtempSync(join(tmpdir(), prefix));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

function executable(path, content = "#!/bin/sh\n") {
  writeFileSync(path, content, { mode: 0o755 });
  return path;
}

function elf(machine) {
  const header = Buffer.alloc(64);
  header.set([0x7f, 0x45, 0x4c, 0x46, 2, 1]);
  header.writeUInt16LE(machine, 18);
  return header;
}

test("remote ARM host skips a desktop bundle and resolves its own PATH", async (t) => {
  const root = temporary(t, "shucked-remote-");
  const extensionPath = join(root, "editors", "vscode");
  const remoteBin = join(root, "remote-bin");
  mkdirSync(join(extensionPath, "bin"), { recursive: true });
  mkdirSync(remoteBin);
  const desktop = Buffer.alloc(32);
  desktop.writeUInt32LE(0xfeedfacf, 0);
  desktop.writeUInt32LE(0x0100000c, 4);
  writeFileSync(join(extensionPath, "bin", "shucked-server"), desktop, { mode: 0o755 });
  const command = join(remoteBin, "shucked-server");
  writeFileSync(command, elf(183), { mode: 0o755 });
  const output = outputChannel();
  const api = resolver({ platform: "linux", arch: "arm64", env: { PATH: remoteBin } });
  const result = await api.resolveServerCommand({ extensionPath }, output, ["--test-arg"]);
  assert.equal(result.command, command);
  assert.ok(output.lines.some(([, message]) => message.includes("Skipping incompatible bundled binary")));
  assert.deepEqual(Array.from(result.args), ["--test-arg"]);
});

test("discovery does not grant executable permissions to a custom binary", { skip: process.platform === "win32" }, (t) => {
  const file = join(temporary(t, "shucked-permissions-"), "custom-server");
  writeFileSync(file, "not executable", { mode: 0o600 });
  assert.equal(resolver().ensureExecutable(file), false);
  assert.equal(statSync(file).mode & 0o777, 0o600);
});

test("bundled binaries may have their executable bit repaired", { skip: process.platform === "win32" }, (t) => {
  const file = join(temporary(t, "shucked-repair-"), "shucked");
  writeFileSync(file, "bundled", { mode: 0o644 });
  assert.equal(resolver().ensureExecutable(file, true), true);
  assert.equal(statSync(file).mode & 0o111, 0o111);
});

test("directories and missing paths are never executables", (t) => {
  const root = temporary(t, "shucked-kinds-");
  const api = resolver();
  assert.equal(api.ensureExecutable(root, true), false);
  assert.equal(api.ensureExecutable(join(root, "missing"), true), false);
});

test("untrusted workspace cannot choose the server program or arguments", async (t) => {
  const root = temporary(t, "shucked-untrusted-");
  const userServer = executable(join(root, "shucked-server-user"), "user selected server");
  const workspaceServer = executable(join(root, "shucked-server-workspace"), "workspace selected server");
  const api = resolver(process, {
    trusted: false,
    values: { "server.path": workspaceServer, "server.extraArgs": ["--workspace-code"] },
    global: { "server.path": userServer, "server.extraArgs": ["--user-option"] },
  });
  const output = outputChannel();
  const server = await api.resolveServerCommand({ extensionPath: root }, output, ["--workspace-code"]);
  assert.equal(server.command, userServer);
  assert.deepEqual(Array.from(server.args), ["--user-option"]);
  assert.equal(await api.resolveBinary({ extensionPath: root }, output), userServer);
});

test("a custom CLI path runs its server subcommand; a dedicated server runs as-is", { skip: process.platform === "win32" }, async (t) => {
  const root = temporary(t, "shucked-custom-");
  const cli = executable(join(root, "shucked"));
  const server = executable(join(root, "shucked-server-custom"));
  for (const [path, expected] of [[cli, ["server", "--extra"]], [server, ["--extra"]]]) {
    const api = resolver(process, { values: { "server.path": path } });
    const result = await api.resolveServerCommand({ extensionPath: root }, outputChannel(), ["--extra"]);
    assert.equal(result.command, path);
    assert.deepEqual(Array.from(result.args), expected);
  }
});

test("a configured path that is not executable is an error, not a fallback", async (t) => {
  const root = temporary(t, "shucked-invalid-");
  const api = resolver(process, { values: { "server.path": join(root, "missing") } });
  await assert.rejects(api.resolveServerCommand({ extensionPath: root }, outputChannel()), /not an executable file/);
  await assert.rejects(api.resolveBinary({ extensionPath: root }, outputChannel()), /not an executable file/);
});

test("workspace builds are preferred over PATH, release before debug", { skip: process.platform === "win32" }, async (t) => {
  const root = temporary(t, "shucked-order-");
  const extensionPath = join(root, "editors", "vscode");
  const onPath = join(root, "path-bin");
  for (const directory of [extensionPath, onPath, join(root, "target", "release"), join(root, "target", "debug")]) {
    mkdirSync(directory, { recursive: true });
  }
  executable(join(onPath, "shucked-server"));
  const environment = { platform: process.platform, arch: process.arch, env: { PATH: onPath } };
  const resolve = async () => (await resolver(environment).resolveServerCommand({ extensionPath }, outputChannel())).command;
  assert.equal(await resolve(), join(onPath, "shucked-server"));
  const debug = executable(join(root, "target", "debug", "shucked"));
  assert.equal(await resolve(), debug);
  const release = executable(join(root, "target", "release", "shucked"));
  assert.equal(await resolve(), release);
  const releaseServer = executable(join(root, "target", "release", "shucked-server"));
  assert.equal(await resolve(), releaseServer);
});

test("nothing found reports how to install or configure the server", async (t) => {
  const root = temporary(t, "shucked-none-");
  const api = resolver({ platform: process.platform, arch: process.arch, env: { PATH: "" } });
  await assert.rejects(api.resolveServerCommand({ extensionPath: join(root, "a", "b") }, outputChannel()), /shucked\.server\.path/);
});

test("findInPath skips empty entries and non-executable candidates", { skip: process.platform === "win32" }, (t) => {
  const root = temporary(t, "shucked-path-");
  const first = join(root, "first"), second = join(root, "second");
  mkdirSync(first);
  mkdirSync(second);
  writeFileSync(join(first, "tool"), "", { mode: 0o644 });
  const tool = executable(join(second, "tool"));
  const api = resolver({ platform: "linux", arch: "x64", env: { PATH: ["", first, second].join(delimiter) } });
  assert.equal(api.findInPath("tool"), tool);
  assert.equal(api.findInPath("absent"), undefined);
  assert.equal(resolver({ platform: "linux", arch: "x64", env: {} }).findInPath("tool"), undefined);
});

test("expandVariables substitutes home and environment variables only when known", () => {
  const api = resolver({ platform: "linux", arch: "x64", env: { SHUCKED_TOOLS: "/opt/tools" } });
  assert.equal(api.expandVariables("~/bin/shucked"), join(homedir(), "bin/shucked"));
  assert.equal(api.expandVariables("$SHUCKED_TOOLS/shucked"), "/opt/tools/shucked");
  assert.equal(api.expandVariables("${SHUCKED_TOOLS}/shucked"), "/opt/tools/shucked");
  assert.equal(api.expandVariables("$UNSET_VARIABLE/shucked"), "$UNSET_VARIABLE/shucked");
  assert.equal(api.expandVariables("/usr/~/shucked"), "/usr/~/shucked", "only a leading tilde means home");
});
