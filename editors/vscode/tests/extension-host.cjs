/* eslint @typescript-eslint/no-require-imports: "off" -- VS Code loads the extension test entry point as CommonJS. */
'use strict';
const vscode = require('vscode');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function eventually(description, operation, timeout = 20000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try { const value = await operation(); if (value) {return value;} } catch (error) { last = error; }
    await delay(150);
  }
  throw new Error(`${description} timed out${last ? `: ${last.message}` : ''}`);
}
const hoverText = content => (typeof content === 'string' ? content : content.value).replaceAll('&nbsp;', ' ');
const diagnosticCode = diagnostic => typeof diagnostic.code === 'object' ? diagnostic.code.value : diagnostic.code;
exports.run = async function run() {
  const report = { vscode: vscode.version, platform: `${process.platform}-${process.arch}`, trusted: vscode.workspace.isTrusted, checks: [], packaged: process.env.SHUCKED_EXTENSION_TEST_PACKAGED === '1', passed: false };
  const check = name => { report.checks.push(name); console.log(`PASS ${name}`); };
  let terminal;
  try {
    const root = vscode.workspace.workspaceFolders[0].uri.fsPath;
    const extension = vscode.extensions.getExtension('fredrir.shucked'); assert.ok(extension, 'development extension registered');
    await extension.activate(); report.extensionPath = extension.extensionPath;
    if (report.packaged) { assert.ok(extension.extensionPath.includes(`${path.sep}extensions${path.sep}`), 'Shucked loaded from isolated installed VSIX'); }
    check('extension activated with isolated configuration');
    assert.equal(vscode.workspace.getConfiguration('shucked').get('history.session'), false);
    assert.equal(vscode.workspace.getConfiguration('shucked').get('history.files'), false); check('history opt-ins default off');
    const uri = vscode.Uri.file(path.join(root, 'smoke.zsh'));
    let document = await vscode.workspace.openTextDocument(uri); await vscode.window.showTextDocument(document);
    report.languageId = document.languageId; report.environment = vscode.workspace.getConfiguration('shucked', document).get('environment');
    report.initialCompletion = (await vscode.commands.executeCommand('vscode.executeCompletionItemProvider', uri, new vscode.Position(3, 14)))?.items?.slice(0, 5).map(item => item.label);
    report.initialHover = (await vscode.commands.executeCommand('vscode.executeHoverProvider', uri, new vscode.Position(2, 4)))?.flatMap(item => item.contents.map(hoverText));
    await eventually('workspace missing command warning', () => vscode.languages.getDiagnostics(uri).some(d => diagnosticCode(d) === 'ENV001' && d.range.start.line === 2)); check('workspace host warning published through VS Code');
    const completion = await eventually('function completion', async () => {
      const result = await vscode.commands.executeCommand('vscode.executeCompletionItemProvider', uri, new vscode.Position(3, 'shucked_smoke_f'.length));
      return result?.items?.find(item => (typeof item.label === 'string' ? item.label : item.label.label) === 'shucked_smoke_function');
    }); assert.ok(completion); check('document function completion');
    const tokens = await eventually('semantic tokens', () => vscode.commands.executeCommand('vscode.provideDocumentSemanticTokens', uri));
    assert.ok(tokens.data.length > 0); assert.ok(Array.from(tokens.data).some((value, index) => index % 5 === 4 && (value & 16) !== 0)); check('invalid command semantic token modifier');
    await vscode.workspace.getConfiguration('shucked', document).update('environment.policy', 'portable', vscode.ConfigurationTarget.Workspace);
    await eventually('portable removes host warnings', () => !vscode.languages.getDiagnostics(uri).some(d => diagnosticCode(d) === 'ENV001')); check('Portable suppresses host absence diagnostics without restart');
    await vscode.workspace.getConfiguration('shucked', document).update('environment.policy', 'workspace', vscode.ConfigurationTarget.Workspace);
    await eventually('workspace checks restored', () => vscode.languages.getDiagnostics(uri).some(d => diagnosticCode(d) === 'ENV001')); check('workspace checks restored');
    const fishUri = vscode.Uri.file(path.join(root, 'smoke.fish'));
    const fish = await vscode.workspace.openTextDocument(fishUri); await vscode.window.showTextDocument(fish);
    assert.equal(fish.languageId, 'fish');
    await eventually('Fish function completion', async () => { const result = await vscode.commands.executeCommand('vscode.executeCompletionItemProvider', fishUri, new vscode.Position(3, 7)); return result?.items?.some(item => (typeof item.label === 'string' ? item.label : item.label.label) === 'fish_fixture'); }); check('Fish registration and native document function completion');
    await vscode.window.showTextDocument(document);
    await vscode.workspace.getConfiguration('shucked').update('history.files', true, vscode.ConfigurationTarget.Global);
    const before = new Set(vscode.window.terminals);
    await vscode.commands.executeCommand('shucked.createTerminal', 'zsh');
    terminal = await eventually('Shucked terminal created', () => vscode.window.terminals.find(item => !before.has(item)));
    assert.ok(terminal.name.startsWith('Shucked'));
    const aliasEdit = new vscode.WorkspaceEdit(); aliasEdit.replace(uri, new vscode.Range(0, 0, document.lineCount, 0), 'shucked_smoke_alias hello\n');
    assert.ok(await vscode.workspace.applyEdit(aliasEdit));
    await eventually('terminal alias hover', async () => {
      const hover = await vscode.commands.executeCommand('vscode.executeHoverProvider', uri, new vscode.Position(0, 4));
      report.lastSessionHover = hover?.flatMap(item => item.contents.map(hoverText));
      return hover?.some(item => item.contents.some(content => /Resolution: Builtin/.test(hoverText(content)) && /InteractiveSession/.test(hoverText(content))));
    }, 30000); check('real terminal prompt hook resolves a fixture startup alias');
    const historyPrefix = 'printf shucked_h';
    const historyEdit = new vscode.WorkspaceEdit(); historyEdit.replace(uri, new vscode.Range(0, 0, document.lineCount, 0), historyPrefix);
    assert.ok(await vscode.workspace.applyEdit(historyEdit));
    const editor = await vscode.window.showTextDocument(document); editor.selection = new vscode.Selection(0, historyPrefix.length, 0, historyPrefix.length);
    await eventually('custom history inline acceptance', async () => {
      await vscode.commands.executeCommand('editor.action.inlineSuggest.trigger'); await delay(150);
      await vscode.commands.executeCommand('editor.action.inlineSuggest.commit');
      return document.getText() === 'printf shucked_history_fixture';
    }); check('custom history path supplies an inline suggestion that only inserts text');
    const reset = new vscode.WorkspaceEdit(); reset.replace(uri, new vscode.Range(0, 0, document.lineCount, 0), historyPrefix); await vscode.workspace.applyEdit(reset);
    editor.selection = new vscode.Selection(0, historyPrefix.length, 0, historyPrefix.length);
    await vscode.commands.executeCommand('editor.action.inlineSuggest.trigger'); await delay(200);
    await vscode.workspace.getConfiguration('shucked').update('history.files', false, vscode.ConfigurationTarget.Global); await delay(200);
    await vscode.commands.executeCommand('editor.action.inlineSuggest.commit');
    assert.equal(document.getText(), historyPrefix); check('history opt-out revokes an already displayed suggestion');
    const restoreAlias = new vscode.WorkspaceEdit(); restoreAlias.replace(uri, new vscode.Range(0, 0, document.lineCount, 0), 'shucked_smoke_alias hello\n'); await vscode.workspace.applyEdit(restoreAlias);
    terminal.dispose(); terminal = undefined;
    await eventually('detached session hover', async () => { const hover = await vscode.commands.executeCommand('vscode.executeHoverProvider', uri, new vscode.Position(0, 4)); return hover?.some(item => item.contents.some(content => /stale|refresh|unavailable|detached/i.test(hoverText(content)))); }); check('terminal exit revokes current session evidence');
    report.passed = true;
  } catch (error) { report.error = error.stack ?? String(error); throw error; }
  finally { report.diagnostics = vscode.languages.getDiagnostics().map(([uri, items]) => ({ file: uri.fsPath, diagnostics: items.map(item => ({code:diagnosticCode(item),message:item.message,line:item.range.start.line})) })); terminal?.dispose(); await fs.writeFile(process.env.SHUCKED_EXTENSION_TEST_RESULT, JSON.stringify(report, null, 2)); }
};
