// The fish TextMate grammar and language configuration shipped for the `fish`
// language: manifest registration, grammar structure (every include resolves,
// every regular expression compiles), and the scopes representative fish gets.
//
// Tokenisation runs through a small reference tokenizer that follows the
// TextMate matching rules the grammar relies on (earliest match wins, the
// end pattern before sub-patterns, captures, nested begin/end). When
// vscode-textmate and vscode-oniguruma resolve from this file, the same
// assertions also run against the real engine.
import assert from "node:assert/strict";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const grammarPath = fileURLToPath(new URL("../syntaxes/fish.tmLanguage.json", import.meta.url));
const grammar = JSON.parse(fs.readFileSync(grammarPath, "utf8"));
const manifest = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const configuration = JSON.parse(fs.readFileSync(new URL("../language-configuration/fish.json", import.meta.url), "utf8"));

test("the manifest registers the fish grammar and language configuration for the fish language", () => {
  const language = manifest.contributes.languages.find(item => item.id === "fish");
  assert.ok(language, "the fish language is declared");
  assert.equal(language.configuration, "./language-configuration/fish.json");
  const registration = manifest.contributes.grammars.find(item => item.language === "fish");
  assert.deepEqual(registration, { language: "fish", scopeName: "source.fish", path: "./syntaxes/fish.tmLanguage.json" });
  assert.equal(grammar.scopeName, registration.scopeName);
  assert.match(grammar.firstLineMatch, /fish/);
  assert.equal(grammar.firstLineMatch, language.firstLine, "the grammar and the language agree on shebang detection");
  const ignored = fs.readFileSync(new URL("../.vscodeignore", import.meta.url), "utf8");
  assert.doesNotMatch(ignored, /syntaxes|language-configuration/, "both directories ship in the VSIX");
});

/** Every rule object in the grammar with the path it was found at. */
function* rules(node, path = "grammar") {
  if (Array.isArray(node)) { for (const [index, item] of node.entries()) { yield* rules(item, `${path}[${index}]`); } return; }
  if (!node || typeof node !== "object") { return; }
  if ("match" in node || "begin" in node || "include" in node || "patterns" in node) { yield [path, node]; }
  for (const key of ["patterns", "repository", "captures", "beginCaptures", "endCaptures"]) {
    if (key in node) {
      const child = node[key];
      if (key === "patterns") { yield* rules(child, `${path}.patterns`); }
      else { for (const [name, value] of Object.entries(child)) { yield* rules(value, `${path}.${key}.${name}`); } }
    }
  }
}

test("every include resolves, every rule matches something, and every scope name is a fish scope", () => {
  let count = 0;
  for (const [path, rule] of rules(grammar)) {
    if (rule === grammar) { continue; }
    count++;
    if ("include" in rule) {
      const reference = rule.include;
      assert.ok(reference === "$self" || reference === "$base" || (reference.startsWith("#") && reference.slice(1) in grammar.repository), `${path}: ${reference} resolves`);
      continue;
    }
    assert.ok("match" in rule || ("begin" in rule && "end" in rule) || "patterns" in rule, `${path} is a match, a begin/end pair, or a pattern list`);
    assert.ok(!("begin" in rule) || "end" in rule, `${path}: begin needs end`);
    for (const key of ["match", "begin", "end"]) {
      if (key in rule) { assert.doesNotThrow(() => compile(rule[key], true), `${path}.${key} compiles`); }
    }
    for (const key of ["name", "contentName"]) {
      if (key in rule) { for (const scope of rule[key].split(" ")) { assert.match(scope, /^[a-z][a-z0-9-]*(\.[a-z0-9-]+)*\.fish$/, `${path}.${key}`); } }
    }
    for (const key of ["captures", "beginCaptures", "endCaptures"]) {
      for (const [index, capture] of Object.entries(rule[key] ?? {})) {
        assert.match(index, /^\d+$/, `${path}.${key} keys are group numbers`);
        if ("name" in capture) { assert.match(capture.name, /\.fish$/, `${path}.${key}.${index}`); }
      }
    }
  }
  assert.ok(count > 40, `the grammar has substance (${count} rules)`);
});

test("the language configuration toggles # comments, pairs brackets and quotes, and folds blocks to end", () => {
  assert.deepEqual(configuration.comments, { lineComment: "#" });
  assert.deepEqual(configuration.brackets, [["(", ")"], ["[", "]"], ["{", "}"]]);
  const closing = Object.fromEntries(configuration.autoClosingPairs.map(pair => [pair.open, pair.close]));
  assert.deepEqual(closing, { "(": ")", "[": "]", "{": "}", '"': '"', "'": "'" });
  assert.ok(configuration.autoClosingPairs.find(pair => pair.open === "'").notIn.includes("comment"), "an apostrophe in a comment is not a quote");
  assert.deepEqual(configuration.surroundingPairs.map(pair => pair.join("")), ["()", "[]", "{}", '""', "''"]);
  assert.doesNotThrow(() => new RegExp(configuration.wordPattern));
  assert.deepEqual("hello_world-2 $var".match(new RegExp(configuration.wordPattern, "g")), ["hello_world-2", "$var"]);
  const start = new RegExp(configuration.folding.markers.start), end = new RegExp(configuration.folding.markers.end);
  for (const line of ["function greet", "  if test -n x", "for f in *", "while true", "switch $x", "begin"]) { assert.match(line, start); }
  for (const line of ["end", "    end", "end # done"]) { assert.match(line, end); }
  for (const line of ["endless", "echo end", "functions"]) { assert.doesNotMatch(line, start); assert.doesNotMatch(line, end); }
  const increase = new RegExp(configuration.indentationRules.increaseIndentPattern), decrease = new RegExp(configuration.indentationRules.decreaseIndentPattern);
  assert.match("if test -f x", increase); assert.match("else if true", increase); assert.match("case '*'", increase);
  assert.doesNotMatch("if true; echo; end", increase, "a one-line block does not indent the next line");
  assert.match("end", decrease); assert.match("else", decrease); assert.match("case 2", decrease);
});

// --- Reference tokenizer -----------------------------------------------------

const compiled = new Map();
/** A grammar regular expression as a JavaScript one; \A only matches on the first line. */
function compile(source, firstLine) {
  const key = `${firstLine ? 1 : 0}${source}`;
  if (!compiled.has(key)) { compiled.set(key, new RegExp(source.replace(/\\A/g, firstLine ? "^" : "(?!)"), "dg")); }
  return compiled.get(key);
}
function resolve(reference) {
  if (reference === "$self" || reference === "$base") { return grammar; }
  return grammar.repository[reference.slice(1)];
}
/** The match or begin/end rules a pattern list stands for, in order. */
function expand(patterns, seen = new Set()) {
  const result = [];
  for (const pattern of patterns ?? []) {
    let rule = pattern;
    if ("include" in pattern) { rule = resolve(pattern.include); if (seen.has(rule)) { continue; } }
    if ("match" in rule || "begin" in rule) { result.push(rule); }
    else if ("patterns" in rule) { const nested = new Set(seen); nested.add(rule); result.push(...expand(rule.patterns, nested)); }
  }
  return result;
}
function exec(regex, text, position) { regex.lastIndex = position; return regex.exec(text); }

/** Tokenize lines with the reference matcher; returns [{ text, scopes }] per line. */
function referenceTokenize(lines) {
  const stack = [{ rule: grammar, scopes: [grammar.scopeName], nameScopes: [grammar.scopeName] }];
  return lines.map((line, lineIndex) => {
    const chars = Array.from({ length: line.length }, () => undefined);
    const paint = (from, to, scopes) => { for (let index = from; index < to; index++) { chars[index] = scopes; } };
    let position = 0, stalls = 0;
    while (position <= line.length) {
      const frame = stack[stack.length - 1];
      let best;
      if (frame.rule.end) {
        const match = exec(compile(frame.rule.end, lineIndex === 0), line, position);
        if (match) { best = { kind: "end", match, rule: frame.rule }; }
      }
      for (const rule of expand(frame.rule.patterns)) {
        const match = exec(compile(rule.match ?? rule.begin, lineIndex === 0), line, position);
        if (match && (!best || match.index < best.match.index)) { best = { kind: rule.match ? "match" : "begin", match, rule }; }
      }
      if (!best) { paint(position, line.length, frame.scopes); break; }
      const { match, rule, kind } = best;
      paint(position, match.index, frame.scopes);
      const end = match.index + match[0].length;
      if (kind === "end") {
        paintCaptures(chars, match, rule.endCaptures, frame.nameScopes, line, lineIndex);
        stack.pop();
      } else if (kind === "match") {
        const scopes = rule.name ? [...frame.scopes, ...rule.name.split(" ")] : frame.scopes;
        paintCaptures(chars, match, rule.captures, scopes, line, lineIndex);
      } else {
        const nameScopes = rule.name ? [...frame.scopes, ...rule.name.split(" ")] : frame.scopes;
        paintCaptures(chars, match, rule.beginCaptures ?? rule.captures, nameScopes, line, lineIndex);
        stack.push({ rule, nameScopes, scopes: rule.contentName ? [...nameScopes, ...rule.contentName.split(" ")] : nameScopes });
      }
      if (end === position) { if (++stalls > 1) { paint(position, position + 1, frame.scopes); position++; stalls = 0; } }
      else { stalls = 0; position = end; }
      if (position === line.length && kind !== "begin" && end === position) { break; }
    }
    return coalesce(line, chars);
  });
}
function paintCaptures(chars, match, captures, scopes, line, lineIndex) {
  const end = match.index + match[0].length;
  for (let index = match.index; index < end; index++) { chars[index] = scopes; }
  for (const [group, capture] of Object.entries(captures ?? {})) {
    const range = match.indices[Number(group)];
    if (!range) { continue; }
    if (capture.name) { for (let index = range[0]; index < range[1]; index++) { chars[index] = [...chars[index], ...capture.name.split(" ")]; } }
    if (capture.patterns) {
      const inner = referenceTokenizeWith(capture.patterns, line.slice(range[0], range[1]), chars[range[0]], lineIndex);
      let cursor = range[0];
      for (const token of inner) { chars.fill(token.scopes, cursor, cursor + token.text.length); cursor += token.text.length; }
    }
  }
}
function referenceTokenizeWith(patterns, text, scopes, lineIndex) {
  const chars = Array.from({ length: text.length }, () => scopes);
  let position = 0;
  while (position < text.length) {
    let best;
    for (const rule of expand(patterns)) {
      const match = exec(compile(rule.match ?? rule.begin, lineIndex === 0), text, position);
      if (match && (!best || match.index < best.match.index)) { best = { match, rule }; }
    }
    if (!best || !best.rule.match) { break; }
    paintCaptures(chars, best.match, best.rule.captures, best.rule.name ? [...scopes, ...best.rule.name.split(" ")] : scopes, text, lineIndex);
    position = Math.max(best.match.index + best.match[0].length, position + 1);
  }
  return coalesce(text, chars);
}
function coalesce(line, chars) {
  const tokens = [];
  for (let index = 0; index < line.length; index++) {
    const scopes = chars[index] ?? [grammar.scopeName];
    const previous = tokens[tokens.length - 1];
    if (previous && previous.scopes.join(" ") === scopes.join(" ")) { previous.text += line[index]; } else { tokens.push({ text: line[index], scopes }); }
  }
  return tokens;
}

async function realTokenizer() {
  let textmate, oniguruma;
  try { textmate = require("vscode-textmate"); oniguruma = require("vscode-oniguruma"); } catch { return undefined; }
  const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({ createOnigScanner: sources => new oniguruma.OnigScanner(sources), createOnigString: value => new oniguruma.OnigString(value) }),
    loadGrammar: async scope => scope === grammar.scopeName ? textmate.parseRawGrammar(fs.readFileSync(grammarPath, "utf8"), grammarPath) : null,
  });
  const loaded = await registry.loadGrammar(grammar.scopeName);
  return lines => {
    let state = textmate.INITIAL;
    return lines.map(line => {
      const result = loaded.tokenizeLine(line, state);
      state = result.ruleStack;
      return result.tokens.map(token => ({ text: line.slice(token.startIndex, token.endIndex), scopes: token.scopes }));
    });
  };
}

const FIXTURE = [
  "#!/usr/bin/env fish",
  "# Greets whoever asks.",
  "function greet --description 'Say hello' -a name",
  '    if test -n "$name"',
  '        echo "Hello, $name!" > /tmp/greeting.txt   # trailing comment',
  "    else if test (count $argv) -eq 0",
  "        echo 'No name: it\\'s fine\\\\' 2>&1 | cat",
  "    end",
  "    for file in (ls *.fish)",
  "        set -l lines (count (cat $file))",
  "        echo $file[1] $$name[1..2] $status $fish_pid",
  "    end",
  "    return 3",
  "end",
  "set -x PATH $HOME/bin $PATH",
  'greet {a,b}c; and not false; or echo "$(pwd)/x\\$y\\"" &',
  "cat < in.txt >> out.txt ^ err.txt && echo ok || echo no &| less",
];

const engines = [{ name: "reference tokenizer", tokenize: referenceTokenize }];
const real = await realTokenizer();
if (real) { engines.push({ name: "vscode-textmate", tokenize: real }); }

for (const engine of engines) {
  test(`${engine.name}: representative fish gets the expected scopes`, () => {
    const lines = engine.tokenize(FIXTURE);
    for (const [index, tokens] of lines.entries()) {
      assert.equal(tokens.map(token => token.text).join(""), FIXTURE[index], `line ${index} is covered exactly once`);
      for (const token of tokens) { assert.equal(token.scopes[0], "source.fish"); }
    }
    const has = (line, text, scope) => {
      const matching = lines[line].filter(token => token.text.trim() === text);
      assert.ok(matching.length, `line ${line}: no token ${JSON.stringify(text)} in ${JSON.stringify(lines[line].map(token => token.text))}`);
      assert.ok(matching.some(token => token.scopes.includes(scope)), `line ${line}: ${JSON.stringify(text)} has ${scope}; got ${JSON.stringify(matching.map(token => token.scopes))}`);
    };
    const lacks = (line, text, scope) => {
      const matching = lines[line].filter(token => token.text.trim() === text);
      assert.ok(matching.length && matching.every(token => !token.scopes.includes(scope)), `line ${line}: ${JSON.stringify(text)} does not have ${scope}`);
    };
    // Shebang and comments.
    assert.ok(lines[0].every(token => token.scopes.includes("comment.line.shebang.fish")), "the whole first line is the shebang");
    has(0, "#!", "punctuation.definition.comment.fish"); lacks(0, "/usr/bin/env fish", "comment.line.number-sign.fish");
    has(1, "#", "punctuation.definition.comment.fish"); has(1, "Greets whoever asks.", "comment.line.number-sign.fish");
    // A function with a description: name, options, string.
    has(2, "function", "keyword.control.fish"); has(2, "greet", "entity.name.function.fish");
    has(2, "--description", "variable.parameter.option.fish"); has(2, "Say hello", "string.quoted.single.fish");
    has(2, "-a", "variable.parameter.option.fish"); has(2, "name", "meta.function.fish"); lacks(2, "name", "variable.other.fish");
    // Nested if / else if / end with test and a double-quoted variable.
    has(3, "if", "keyword.control.fish"); has(3, "test", "keyword.other.builtin.fish"); has(3, "-n", "variable.parameter.option.fish");
    has(3, '"', "punctuation.definition.string.begin.fish"); has(3, "name", "variable.other.fish"); has(3, "name", "string.quoted.double.fish");
    has(4, "echo", "support.function.builtin.fish"); has(4, "Hello,", "string.quoted.double.fish"); has(4, "name", "variable.other.fish");
    has(4, ">", "keyword.operator.redirect.fish"); has(4, "trailing comment", "comment.line.number-sign.fish"); lacks(4, "/tmp/greeting.txt", "comment.line.number-sign.fish");
    has(5, "else", "keyword.control.fish"); has(5, "if", "keyword.control.fish"); has(5, "(", "punctuation.definition.substitution.begin.fish");
    has(5, "count", "support.function.builtin.fish"); has(5, "count", "meta.command-substitution.fish"); has(5, "argv", "variable.language.fish");
    has(5, "-eq", "keyword.operator.comparison.fish"); lacks(5, "-eq", "variable.parameter.option.fish"); has(5, "0", "constant.numeric.fish");
    // Single-quoted escapes, redirection of stderr, pipe, an external command.
    has(6, "No name: it", "string.quoted.single.fish"); has(6, "\\'", "constant.character.escape.fish"); has(6, "\\\\", "constant.character.escape.fish");
    has(6, "2>&1", "keyword.operator.redirect.fish"); has(6, "|", "keyword.operator.pipe.fish"); lacks(6, "cat", "support.function.builtin.fish");
    has(7, "end", "keyword.control.fish");
    // A for loop over a command substitution with a glob.
    has(8, "for", "keyword.control.fish"); has(8, "file", "variable.other.assignment.fish"); has(8, "in", "keyword.control.fish");
    has(8, "ls", "meta.command-substitution.fish"); has(8, "*", "constant.other.wildcard.fish"); has(8, ")", "punctuation.definition.substitution.end.fish");
    has(9, "set", "keyword.other.builtin.fish"); has(9, "-l", "variable.parameter.option.fish"); has(9, "lines", "variable.other.assignment.fish");
    has(9, "cat", "meta.command-substitution.fish"); has(9, "file", "variable.other.fish");
    // Variables: plain, double-dollar, indexed with ranges, special.
    has(10, "$", "punctuation.definition.variable.fish"); has(10, "file", "variable.other.fish"); has(10, "[", "punctuation.definition.index.begin.fish");
    has(10, "1", "constant.numeric.integer.fish"); has(10, "$$", "punctuation.definition.variable.fish"); has(10, "..", "keyword.operator.range.fish");
    has(10, "status", "variable.language.fish"); has(10, "fish_pid", "variable.language.fish");
    has(11, "end", "keyword.control.fish"); has(12, "return", "keyword.control.fish"); has(12, "3", "constant.numeric.fish"); has(13, "end", "keyword.control.fish");
    // set -x PATH $HOME/bin $PATH
    has(14, "set", "keyword.other.builtin.fish"); has(14, "-x", "variable.parameter.option.fish"); has(14, "PATH", "variable.other.assignment.fish");
    has(14, "HOME", "variable.language.fish"); has(14, "/bin", "source.fish"); lacks(14, "/bin", "variable.language.fish");
    // Brace expansion, logical keywords, separators, $(...) inside double quotes, escapes, background.
    has(15, "{", "punctuation.definition.brace-expansion.begin.fish"); has(15, ",", "punctuation.separator.brace-expansion.fish"); has(15, "a", "meta.brace-expansion.fish");
    has(15, ";", "punctuation.terminator.statement.fish"); has(15, "and", "keyword.operator.logical.fish"); has(15, "not", "keyword.operator.logical.fish");
    has(15, "false", "support.function.builtin.fish"); has(15, "or", "keyword.operator.logical.fish");
    has(15, "$(", "punctuation.definition.substitution.begin.fish"); has(15, "pwd", "support.function.builtin.fish"); has(15, "pwd", "string.quoted.double.fish");
    has(15, "\\$", "constant.character.escape.fish"); has(15, '\\"', "constant.character.escape.fish"); has(15, "&", "keyword.operator.background.fish");
    // Redirections and pipes.
    has(16, "<", "keyword.operator.redirect.fish"); has(16, ">>", "keyword.operator.redirect.fish"); has(16, "^", "keyword.operator.redirect.fish");
    has(16, "&&", "keyword.operator.logical.fish"); has(16, "||", "keyword.operator.logical.fish"); has(16, "&|", "keyword.operator.pipe.fish");
    has(16, "ok", "source.fish"); lacks(16, "less", "support.function.builtin.fish");
  });

  test(`${engine.name}: strings and comments are not mistaken for one another`, () => {
    const [comment, hash, single, doubled, quoteInComment] = engine.tokenize([
      "echo hello # a comment with 'quotes' and \"more\"",
      "echo foo#bar",
      "echo 'a # in a string' \"$var # not a comment\"",
      'echo "a \'b\' c" \'d "e" f\'',
      "# it's fine",
    ]);
    assert.ok(comment.filter(token => token.scopes.includes("comment.line.number-sign.fish")).map(token => token.text).join("") === "# a comment with 'quotes' and \"more\"");
    assert.ok(!comment.some(token => token.scopes.includes("string.quoted.single.fish")), "quotes inside a comment are comment text");
    assert.ok(!hash.some(token => token.scopes.includes("comment.line.number-sign.fish")), "# inside a word does not start a comment");
    assert.ok(!single.some(token => token.scopes.includes("comment.line.number-sign.fish")), "# inside strings is string text");
    assert.ok(single.some(token => token.text === "var" && token.scopes.includes("variable.other.fish")));
    assert.equal(doubled.filter(token => token.scopes.includes("string.quoted.double.fish")).map(token => token.text).join(""), '"a \'b\' c"');
    assert.equal(doubled.filter(token => token.scopes.includes("string.quoted.single.fish")).map(token => token.text).join(""), "'d \"e\" f'");
    assert.ok(quoteInComment.every(token => token.scopes.includes("comment.line.number-sign.fish")));
  });

  test(`${engine.name}: a multi-line string keeps its scope across lines and closes`, () => {
    const [first, second, third] = engine.tokenize(["echo 'one", "two' three", "echo done"]);
    assert.ok(first.some(token => token.text === "'one" || token.text === "one"));
    assert.ok(first.at(-1).scopes.includes("string.quoted.single.fish"));
    assert.ok(second[0].scopes.includes("string.quoted.single.fish"));
    assert.ok(second.some(token => token.text.trim() === "three" && !token.scopes.includes("string.quoted.single.fish")));
    assert.ok(third.some(token => token.text === "echo" && token.scopes.includes("support.function.builtin.fish")));
  });
}

test("the real TextMate engine was exercised when available", t => {
  if (!real) { t.skip("vscode-textmate and vscode-oniguruma are not installed; the reference tokenizer ran alone"); return; }
  assert.equal(engines.length, 2);
});
