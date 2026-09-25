import assert from "node:assert/strict";
import { test } from "node:test";
import { bundle, evaluate } from "./support/load.mjs";
import { configuration } from "./support/fake-vscode.mjs";

const compiled = await bundle("client.ts", ["vscode-languageclient/node"]);

function client({ trusted = true, now = () => 0 } = {}) {
  const clock = { now };
  const FakeDate = class extends Date { static now() { return clock.now(); } };
  return evaluate(compiled, {
    vscode: { workspace: { isTrusted: trusted } },
    modules: { "vscode-languageclient/node": {} },
    globals: { Date: FakeDate },
  });
}

test("native execution permission comes from VS Code trust, not workspace settings", () => {
  for (const isTrusted of [false, true]) {
    const options = client({ trusted: isTrusted }).getInitializationOptions(configuration({ values: { nativeExecutionAllowed: !isTrusted } }));
    assert.equal(options.nativeExecutionAllowed, isTrusted);
  }
});

test("initialization options forward every setting group the server reads", () => {
  const values = { environment: { policy: "portable" }, unsafeFixes: { enable: true }, fixAll: { enable: false }, lint: { enable: true }, format: { enable: true }, codeAction: { disableRuleComment: { enable: false } }, server: { completion: { maxItems: 5 } } };
  const options = client().getInitializationOptions(configuration({ values }));
  assert.deepEqual(Object.keys(options).sort(), ["codeAction", "environment", "fixAll", "format", "lint", "nativeExecutionAllowed", "server", "unsafeFixes"]);
  for (const [key, value] of Object.entries(values)) {
    assert.deepEqual(options[key], value, key);
  }
});

test("five crashes within three minutes are a crash loop", () => {
  let now = 0;
  const tracker = new (client({ now: () => now }).CrashTracker)();
  for (let crash = 1; crash <= 4; crash++) {
    now += 10_000;
    assert.deepEqual({ ...tracker.recordCrash() }, { isCrashLoop: false, crashCount: crash });
  }
  now += 10_000;
  assert.deepEqual({ ...tracker.recordCrash() }, { isCrashLoop: true, crashCount: 5 });
});

test("crashes outside the rolling window are forgotten", () => {
  let now = 0;
  const tracker = new (client({ now: () => now }).CrashTracker)();
  for (let crash = 0; crash < 4; crash++) { tracker.recordCrash(); }
  now += 3 * 60 * 1000;
  assert.deepEqual({ ...tracker.recordCrash() }, { isCrashLoop: false, crashCount: 1 });
});

test("resetting the tracker starts a fresh window", () => {
  const tracker = new (client().CrashTracker)();
  for (let crash = 0; crash < 4; crash++) { tracker.recordCrash(); }
  tracker.reset();
  assert.equal(tracker.recordCrash().crashCount, 1);
});
