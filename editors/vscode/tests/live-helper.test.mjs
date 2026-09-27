// The persistent live completion helper, driven through the real bash hook in
// an interactive bash on a pseudo-terminal, with this test playing the
// extension's part of the session socket. zsh and fish follow the same
// protocol; their hooks are exercised by the pytest suite in
// tests/editors/vscode/shell_integration when those shells are installed.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const integration = fileURLToPath(new URL("../shell-integration/", import.meta.url));
const SESSION_ID = "b".repeat(32);
const SESSION_TOKEN = "c".repeat(64);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
/** Whether a process still runs; a stopped orphan waiting for a lazy init to reap it counts as gone. */
const alive = pid => {
  try { process.kill(pid, 0); } catch { return false; }
  if (process.platform !== "linux") { return true; }
  try { return !/\) Z /.test(fs.readFileSync(`/proc/${pid}/stat`, "utf8")); } catch { return false; }
};
const quote = value => `'${value.replace(/'/g, "'\\''")}'`;

/** Interactive bash needs a terminal: readline decides when traps run. */
function ptyCommand(command) {
  if (process.platform === "darwin") { return ["script", ["-q", "/dev/null", "/bin/sh", "-c", command]]; }
  return ["script", ["-qfec", command, "/dev/null"]];
}

async function skipUnlessSupported(t) {
  if (process.platform === "win32") { t.skip("the live channel is Unix-only"); return false; }
  if (!fs.existsSync("/usr/bin/script")) { t.skip("script(1) is needed for a pseudo-terminal"); return false; }
  return true;
}

/** Starts bash with `script` sourced before the hook; returns the fake extension side. */
async function startBash(t, script) {
  const directory = fs.mkdtempSync(join(tmpdir(), "shucked-live-"));
  fs.chmodSync(directory, 0o700);
  const messages = [];
  let helper;
  let helperClosed = false;
  const socketPath = join(directory, "state.sock");
  const server = net.createServer(socket => {
    let buffer = Buffer.alloc(0);
    socket.on("error", () => socket.destroy());
    socket.on("data", chunk => {
      buffer = Buffer.concat([buffer, chunk]);
      let at;
      while ((at = buffer.indexOf(10)) >= 0) {
        const line = buffer.subarray(0, at); buffer = buffer.subarray(at + 1);
        try {
          const message = JSON.parse(line.toString("utf8"));
          messages.push(message);
          if (message.kind === "liveHelper" && message.phase === "hello") { helper = socket; socket.on("close", () => { helperClosed = true; }); }
        } catch { /* ignore */ }
      }
    });
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(socketPath, resolve); });
  const rc = join(directory, "bashrc");
  fs.writeFileSync(rc, `${script}\nsource ${quote(join(integration, "bash.sh"))}\nPS1='READY> '\n`);
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("SHUCKED_") && !key.startsWith("VSCODE_")));
  Object.assign(environment, {
    HOME: directory, TERM: "dumb",
    SHUCKED_SESSION_SOCKET: socketPath, SHUCKED_SESSION_ID: SESSION_ID, SHUCKED_SESSION_TOKEN: SESSION_TOKEN,
    SHUCKED_LIVE_ALLOWED: "1", SHUCKED_LIVE_DIRECTORY: directory, SHUCKED_LIVE_HELPER: join(integration, "live-helper.cjs"),
    SHUCKED_CAPTURE: join(integration, "capture.cjs"), SHUCKED_NODE: process.execPath,
  });
  const [command, args] = ptyCommand(`bash --noprofile --rcfile ${quote(rc)} -i`);
  const child = spawn(command, args, { env: environment, cwd: directory, stdio: ["pipe", "pipe", "pipe"] });
  let output = "";
  child.stdout.on("data", chunk => { output += chunk; });
  child.stderr.on("data", chunk => { output += chunk; });
  let exited = false;
  child.on("exit", () => { exited = true; });
  const session = {
    directory, messages,
    get output() { return output; },
    get helper() { return helper; },
    get helperClosed() { return helperClosed; },
    get exited() { return exited; },
    async wait(description, condition, timeout = 4000) {
      const deadline = Date.now() + timeout;
      for (;;) {
        const value = condition();
        if (value) { return value; }
        if (Date.now() > deadline) { throw new Error(`${description} timed out; terminal=${JSON.stringify(output.slice(-800))}; messages=${JSON.stringify(messages.map(message => message.phase ?? message.kind ?? "metadata"))}`); }
        await sleep(20);
      }
    },
    latest(predicate) { return [...messages].reverse().find(predicate); },
    metadata(after = 0) { return session.wait("prompt metadata", () => session.latest(message => message.shell === "bash" && message.liveCompletion === true && message.generation > after)); },
    hello() { return session.wait("helper greeting", () => session.latest(message => message.kind === "liveHelper" && message.phase === "hello")); },
    async idle() {
      // The prompt is drawn and nothing follows it for a moment: the shell waits for input.
      const idlePrompt = new RegExp(`READY> [\\s${String.fromCharCode(27)}[\\]0-9;?]*$`);
      for (;;) {
        await session.wait("idle prompt", () => idlePrompt.test(output), 5000);
        const size = output.length; await sleep(250);
        if (output.length === size) { return; }
      }
    },
    request(query, generation, prefix, words) {
      assert.ok(helper && !helper.destroyed, "the helper is connected");
      helper.write(JSON.stringify({ kind: "request", query, generation, prefix, words }) + "\n");
    },
    cancel(query) { helper.write(JSON.stringify({ kind: "cancel", query }) + "\n"); },
    reply(query, timeout = 3000) { return session.wait(`reply to ${query}`, () => session.latest(message => message.query === query && message.phase === "result"), timeout); },
    type(text) { child.stdin.write(text); },
  };
  t.after(async () => {
    try { child.stdin.write("exit\n"); } catch { /* already gone */ }
    await Promise.race([new Promise(resolve => child.on("exit", resolve)), sleep(1500)]);
    try { child.kill("SIGKILL"); } catch { /* already gone */ }
    const greeting = session.latest(message => message.kind === "liveHelper");
    if (greeting && alive(greeting.pid)) { try { process.kill(greeting.pid, "SIGKILL"); } catch { /* already gone */ } }
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(directory, { recursive: true, force: true });
  });
  return session;
}

const STATE = 'my_completion_value=live_first\ncustom() { :; }\n_custom() { COMPREPLY=("$my_completion_value"); }\ncomplete -F _custom custom\n';
// A callback that never finishes and leaves a background child behind; PATH points nowhere.
const HANGING = 'custom() { :; }\n_custom() { echo $BASHPID > "$HOME/worker-pid"; /bin/sleep 30 & echo $! > "$HOME/child-pid"; wait; }\ncomplete -F _custom custom\nPATH=/nonexistent\n';

test("the hook starts one helper per shell, which introduces itself and owns the FIFO", async t => {
  if (!await skipUnlessSupported(t)) { return; }
  const session = await startBash(t, STATE);
  const hello = await session.hello();
  const metadata = await session.metadata();
  assert.equal(hello.shell, "bash");
  assert.equal(hello.signal, "SIGWINCH", "bash requests travel on the resize signal readline dispatches promptly");
  assert.equal(hello.shellPid, metadata.pid, "the helper names its own shell");
  assert.equal(hello.token, SESSION_TOKEN);
  assert.ok(alive(hello.pid), "the helper is running");
  assert.equal(metadata.liveSignal, "SIGWINCH");
  assert.ok(fs.statSync(join(session.directory, "live.fifo")).isFIFO(), "the helper created the record FIFO");
  await sleep(300);
  assert.equal(session.messages.filter(message => message.kind === "liveHelper").length, 1, "exactly one helper");
  assert.equal(session.output.includes("[1]"), false, "no job-control noise reached the terminal");
});

test("a request at the idle prompt is served from current shell state without evaluating editor words", async t => {
  if (!await skipUnlessSupported(t)) { return; }
  const session = await startBash(t, STATE);
  await session.hello();
  let metadata = await session.metadata();
  const marker = join(session.directory, "must-not-exist");
  for (const [index, expected] of ["live_first", "live_second"].entries()) {
    if (index) {
      // Authored input simulates the user changing state; the extension never types.
      session.type("my_completion_value=live_second\n");
      metadata = await session.metadata(metadata.generation);
    }
    await session.idle();
    const query = String(index + 1).repeat(32);
    const started = performance.now();
    session.request(query, metadata.generation, "live", ["custom", `$(touch ${marker})`]);
    const reply = await session.reply(query);
    const elapsed = performance.now() - started;
    assert.deepEqual(reply.candidates.map(item => item.text), [expected]);
    assert.equal(reply.partial, false);
    assert.equal(reply.generation, metadata.generation);
    assert.equal(reply.id, SESSION_ID);
    assert.ok(Number.isSafeInteger(reply.elapsedMs) && reply.elapsedMs <= elapsed + 1, `the helper reports its own timing (${reply.elapsedMs}ms of ${Math.round(elapsed)}ms)`);
    assert.ok(elapsed < 1000, `served while idle, without a keystroke (${Math.round(elapsed)}ms)`);
    assert.equal(fs.existsSync(marker), false, "editor words were executed");
  }
  const prompts = session.messages.filter(message => "cwd" in message);
  assert.ok(prompts.length >= 2 && prompts.every(message => message.pid === metadata.pid), "a private worker reported prompt metadata");
  assert.equal(fs.existsSync(join(session.directory, "request")), false, "served requests are cleaned up");
});

test("a hanging completer is stopped at the deadline, reported as partial, and the next request still works", async t => {
  if (!await skipUnlessSupported(t)) { return; }
  const session = await startBash(t, HANGING);
  await session.hello();
  let metadata = await session.metadata();
  await session.idle();
  const query = "e".repeat(32);
  const started = performance.now();
  session.request(query, metadata.generation, "live", ["custom"]);
  const reply = await session.reply(query);
  assert.equal(reply.partial, true);
  assert.match(reply.reason, /timed out/);
  assert.ok(performance.now() - started < 1600, "the helper's own deadline applies");
  const files = ["worker-pid", "child-pid"].map(name => join(session.directory, name));
  await session.wait("worker and child pids recorded", () => files.every(file => fs.existsSync(file) && fs.readFileSync(file, "utf8").trim()));
  const [worker, child] = files.map(file => Number(fs.readFileSync(file, "utf8")));
  await session.wait("worker and child stopped", () => !alive(worker) && !alive(child), 3000);
  assert.equal(session.exited, false, "the interactive shell itself survives");
  // Recovery: a fast completer defined afterwards is served normally.
  session.type("_custom() { COMPREPLY=(recovered); }\n");
  metadata = await session.metadata(metadata.generation);
  await session.idle();
  session.request("f".repeat(32), metadata.generation, "", ["custom"]);
  const next = await session.reply("f".repeat(32));
  assert.deepEqual(next.candidates.map(item => item.text), ["recovered"]);
  assert.equal(next.partial, false);
});

test("cancellation stops the worker and its children at once and produces no result", async t => {
  if (!await skipUnlessSupported(t)) { return; }
  const session = await startBash(t, HANGING);
  await session.hello();
  const metadata = await session.metadata();
  await session.idle();
  const query = "d".repeat(32);
  session.request(query, metadata.generation, "live", ["custom"]);
  const files = ["worker-pid", "child-pid"].map(name => join(session.directory, name));
  await session.wait("worker started", () => files.every(file => fs.existsSync(file) && fs.readFileSync(file, "utf8").trim()));
  const [worker, child] = files.map(file => Number(fs.readFileSync(file, "utf8")));
  const cancelled = performance.now();
  session.cancel(query);
  await session.wait("worker and child stopped", () => !alive(worker) && !alive(child), 1000);
  assert.ok(performance.now() - cancelled < 1000, "well before the deadline");
  await sleep(300);
  assert.equal(session.latest(message => message.query === query), undefined, "a cancelled request is not answered");
  assert.equal(session.exited, false);
});

test("a stop frame from the extension ends the helper without touching the shell", async t => {
  if (!await skipUnlessSupported(t)) { return; }
  const session = await startBash(t, STATE);
  const hello = await session.hello();
  await session.metadata();
  session.helper.write(JSON.stringify({ kind: "stop" }) + "\n");
  await session.wait("helper exited", () => !alive(hello.pid), 3000);
  assert.equal(fs.existsSync(join(session.directory, "live.fifo")), false, "the FIFO is removed on exit");
  assert.equal(session.exited, false, "the shell keeps running");
});

test("the helper exits when its shell exits", async t => {
  if (!await skipUnlessSupported(t)) { return; }
  const session = await startBash(t, STATE);
  const hello = await session.hello();
  await session.metadata();
  session.type("exit\n");
  await session.wait("shell exited", () => session.exited, 5000);
  await session.wait("helper exited", () => !alive(hello.pid) && session.helperClosed, 3000);
  assert.equal(fs.existsSync(join(session.directory, "live.fifo")), false, "the FIFO is removed on exit");
});

test("an existing resize trap is kept and live completion is declined", async t => {
  if (!await skipUnlessSupported(t)) { return; }
  const session = await startBash(t, `trap 'builtin printf occupied > "$HOME/trap-called"' WINCH\n${STATE}`);
  const metadata = await session.wait("prompt metadata", () => session.latest(message => message.shell === "bash"));
  assert.equal(metadata.liveCompletion, false, "no signal was reserved");
  await sleep(500);
  assert.equal(session.latest(message => message.kind === "liveHelper"), undefined, "no helper was started");
  await session.idle();
  process.kill(metadata.pid, "SIGWINCH");
  await session.wait("the user's own trap ran", () => fs.existsSync(join(session.directory, "trap-called")), 3000);
  assert.equal(session.exited, false);
});
