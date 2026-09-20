/* eslint @typescript-eslint/no-require-imports: "off" -- Loaded by the extension host. */
'use strict';
const vscode = require('vscode');
const assert = require('node:assert/strict');
const path = require('node:path');

// Observe actual editor insertion after automatic popup; do not invoke completion
// providers or triggerSuggest, which would hide missing trigger registrations.
exports.checkAutomaticCompletion = async function checkAutomaticCompletion(root, report, eventually) {
  const folder = 'aaa_completion_fixture';
  await vscode.workspace.fs.createDirectory(vscode.Uri.file(path.join(root, folder)));
  const uri = vscode.Uri.file(path.join(root, 'automatic-completion.zsh'));
  await vscode.workspace.fs.writeFile(uri, Buffer.from('#!/bin/zsh\ncd'));
  const document = await vscode.workspace.openTextDocument(uri);
  await vscode.workspace.getConfiguration('editor', document).update('wordBasedSuggestions', 'off', vscode.ConfigurationTarget.Workspace);
  const editor = await vscode.window.showTextDocument(document);
  await vscode.commands.executeCommand('workbench.action.focusActiveEditorGroup');
  report.automaticCompletion = [];
  const seen = new Set();
  for (const [before, typed, pattern] of [
    ['cd', ' ', /^cd \S+\/$/],
    ['ls ', '-', /^ls -\S+$/],
    ['ls ', '-', /^ls -\S+$/],
    ['docker', ' ', /^docker (?:attach|build|builder|--config|--debug)$/],
    ['docker', ' ', /^docker (?:attach|build|builder|--config|--debug)$/],
  ]) {
    const warm = seen.has(before);
    if (before === 'docker' && !warm) {
      // An empty launch directory prevents unrelated fallback paths from making
      // this native subcommand/flag popup check pass before the provider is ready.
      const cwd = path.join(root, '..', 'completion-empty');
      await vscode.workspace.fs.createDirectory(vscode.Uri.file(cwd));
      await vscode.workspace.getConfiguration('shucked', document).update('environment.cwd', cwd, vscode.ConfigurationTarget.Workspace);
      await vscode.commands.executeCommand('shucked.restartServer');
    }
    await vscode.commands.executeCommand('shucked.dismissCompletion');
    const edit = new vscode.WorkspaceEdit();
    edit.replace(uri, new vscode.Range(0, 0, document.lineCount, 0), `#!/bin/zsh\n${before}`);
    assert.ok(await vscode.workspace.applyEdit(edit));
    editor.selection = new vscode.Selection(1, before.length, 1, before.length);
    const started = Date.now();
    await vscode.commands.executeCommand('type', { text: typed });
    const result = await eventually(`automatic popup after ${JSON.stringify(before + typed)}`, async () => {
      await vscode.commands.executeCommand('acceptSelectedSuggestion');
      const line = document.lineAt(1).text;
      return line !== before + typed ? line : undefined;
    }, 10000);
    assert.match(result, pattern);
    // This includes polling and acceptance IPC; server response latency is traced separately.
    report.automaticCompletion.push({ typed: before + typed, warm, inserted: result, acceptanceMs: Date.now() - started });
    seen.add(before);
  }
  await vscode.workspace.getConfiguration('shucked', document).update('environment.cwd', undefined, vscode.ConfigurationTarget.Workspace);
};
