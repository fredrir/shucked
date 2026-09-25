// Loads extension sources for unit tests. Each source file is bundled once with
// esbuild; every load evaluates it in a fresh context, so a test can substitute
// the `vscode` API, Node modules, and globals such as `process` or timers.
import { build } from "esbuild";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

const require = createRequire(import.meta.url);
const bundles = new Map();

/** Bundle `src/<source>` (e.g. "binary.ts"); `vscode` and `externals` stay unresolved. */
export function bundle(source, externals = []) {
  const key = [source, ...externals].join("\0");
  if (!bundles.has(key)) {
    const filename = fileURLToPath(new URL(`../../src/${source}`, import.meta.url));
    bundles.set(key, build({
      entryPoints: [filename],
      bundle: true,
      platform: "node",
      format: "cjs",
      external: ["vscode", ...externals],
      sourcemap: "inline",
      write: false,
    }).then(result => ({ code: result.outputFiles[0].text, filename })));
  }
  return bundles.get(key);
}

/**
 * Evaluate a bundle and return its exports.
 * @param {{code: string, filename: string}} compiled
 * @param {{vscode?: object, modules?: Record<string, unknown>, globals?: Record<string, unknown>}} options
 */
export function evaluate(compiled, { vscode = {}, modules = {}, globals = {} } = {}) {
  const module = { exports: {} };
  const context = {
    module, exports: module.exports, Buffer, process, console, performance,
    setTimeout, clearTimeout, setInterval, clearInterval, setImmediate,
    ...globals,
    require: id => id === "vscode" ? vscode : Object.hasOwn(modules, id) ? modules[id] : require(id),
  };
  runInNewContext(compiled.code, context, { filename: compiled.filename });
  return module.exports;
}

/** Bundle and evaluate in one step. */
export async function load(source, options = {}, externals = []) {
  return evaluate(await bundle(source, externals), options);
}
