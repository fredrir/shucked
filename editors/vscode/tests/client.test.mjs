import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const compiled = await build({
  entryPoints: [fileURLToPath(new URL("../src/client.ts", import.meta.url))],
  bundle: true,
  platform: "node",
  format: "cjs",
  external: ["vscode", "vscode-languageclient/node"],
  write: false,
});

test("native execution permission comes from VS Code trust, not workspace settings", () => {
  for (const isTrusted of [false, true]) {
    const module = { exports: {} };
    runInNewContext(compiled.outputFiles[0].text, {
      module,
      exports: module.exports,
      Buffer,
      process,
      require: (id) => {
        if (id === "vscode") {
          return { workspace: { isTrusted } };
        }
        if (id === "vscode-languageclient/node") {
          return {};
        }
        return require(id);
      },
    });
    const options = module.exports.getInitializationOptions({ get: () => true });
    assert.equal(options.nativeExecutionAllowed, isTrusted);
  }
});
