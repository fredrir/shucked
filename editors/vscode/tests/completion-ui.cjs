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
  assert.equal(vscode.workspace.getConfiguration('editor', document).get('wordBasedSuggestions'), 'off', 'shipped shell defaults suppress unrelated document-word guesses');
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

// Keep the inspected workspace read-only: all typing is in one untitled buffer.
exports.checkCompletionRegression = async function checkCompletionRegression(root, report, eventually) {
  const workspaceFiles = new Set((await vscode.workspace.fs.readDirectory(vscode.Uri.file(root)))
    .filter(([, type]) => type === vscode.FileType.File).map(([name]) => name));
  const document = await vscode.workspace.openTextDocument({ language: 'shellscript', content: '#!/bin/zsh\n# aaa_word_guess_from_comment\nbrew' });
  const editor = await vscode.window.showTextDocument(document);
  const configuration = vscode.workspace.getConfiguration('editor', document);
  const setting = configuration.inspect('wordBasedSuggestions');
  assert.equal(configuration.get('wordBasedSuggestions'), 'off');
  assert.equal(setting.workspaceValue, undefined, 'no test-only word suggestion override');
  assert.equal(setting.globalValue, undefined, 'no personal setting mutation');
  const plain = await vscode.workspace.openTextDocument({ language: 'plaintext', content: '' });
  assert.notEqual(vscode.workspace.getConfiguration('editor', plain).get('wordBasedSuggestions'), 'off', 'other languages keep ordinary defaults');
  report.completionRegression = { workspace: root, wordSuggestions: configuration.get('wordBasedSuggestions'), scenarios: [] };
  for (const [before, typed, pattern] of [
    ['brew', ' ', /^brew (?:--[\w-]+|[\w-]+)$/],
    ['ls ', '-', /^ls -\S+$/],
  ]) {
    await vscode.commands.executeCommand('shucked.dismissCompletion');
    const edit = new vscode.WorkspaceEdit();
    edit.replace(document.uri, new vscode.Range(0, 0, document.lineCount, 0), `#!/bin/zsh\n# aaa_word_guess_from_comment\n${before}`);
    assert.ok(await vscode.workspace.applyEdit(edit));
    editor.selection = new vscode.Selection(2, before.length, 2, before.length);
    await vscode.commands.executeCommand('workbench.action.focusActiveEditorGroup');
    const started = Date.now();
    await vscode.commands.executeCommand('type', { text: typed });
    const inserted = await eventually(`ordinary automatic ${before + typed}`, async () => {
      await vscode.commands.executeCommand('acceptSelectedSuggestion');
      const line = document.lineAt(2).text;
      return line === before + typed ? undefined : line;
    }, 8000);
    assert.match(inserted, pattern);
    assert.doesNotMatch(inserted, /aaa_word_guess|\.sh\b|\/$/, 'a shell definition must win over document words and workspace files');
    assert.equal(workspaceFiles.has(inserted.slice(before.length + typed.length)), false, 'workspace files are not command subcommands');
    report.completionRegression.scenarios.push({ typed: before + typed, inserted, acceptanceMs: Date.now() - started });
  }
  await vscode.commands.executeCommand('shucked.dismissCompletion');
  const edit = new vscode.WorkspaceEdit();
  edit.replace(document.uri, new vscode.Range(0, 0, document.lineCount, 0), '#!/bin/zsh\n# aaa_word_guess_from_comment\ncd');
  assert.ok(await vscode.workspace.applyEdit(edit));
  editor.selection = new vscode.Selection(2, 2, 2, 2);
  await vscode.commands.executeCommand('type', { text: ' ' });
  const samples = [];
  await eventually('directory completion settles without files or word guesses', async () => {
    const list = await vscode.commands.executeCommand('vscode.executeCompletionItemProvider', document.uri, new vscode.Position(2, 3));
    const labels = (list?.items ?? []).map(item => typeof item.label === 'string' ? item.label : item.label.label);
    samples.push({ labels, irrelevant: (list?.items ?? []).some(item => item.kind === vscode.CompletionItemKind.File || item.kind === vscode.CompletionItemKind.Text)
      || labels.some(label => label.includes('aaa_word_guess') || workspaceFiles.has(label)) });
    return list && !list.isIncomplete;
  }, 8000);
  assert.equal(samples.some(sample => sample.irrelevant), false, `every intermediate cd response must be contextual: ${JSON.stringify(samples)}`);
  await vscode.commands.executeCommand('acceptSelectedSuggestion');
  const inserted = document.lineAt(2).text;
  assert.ok(inserted === 'cd ' || /^cd \S+\/$/.test(inserted), `cd inserted unrelated text: ${inserted}`);
  report.completionRegression.scenarios.push({ typed: 'cd ', inserted, samples });
  report.completionRegression.snippets = [];
  for (const [keyword, first, second, structure] of [
    ['if', 'condition', ':', /^if condition; then\n\s+:\nfi\n$/],
    ['for', 'item', 'items', /^for item in items; do\n\s+:\ndone\n$/],
  ]) {
    await vscode.commands.executeCommand('shucked.dismissCompletion');
    const reset = new vscode.WorkspaceEdit();
    reset.replace(document.uri, new vscode.Range(0, 0, document.lineCount, 0), '#!/bin/zsh\n');
    assert.ok(await vscode.workspace.applyEdit(reset));
    editor.selection = new vscode.Selection(1, 0, 1, 0);
    await vscode.commands.executeCommand('type', { text: keyword });
    const block = await eventually(`automatic ${keyword} block snippet`, async () => {
      await vscode.commands.executeCommand('acceptSelectedSuggestion');
      return document.lineCount > 2 ? document.getText().slice('#!/bin/zsh\n'.length) : undefined;
    }, 8000);
    assert.match(block, structure);
    assert.equal(document.getText(editor.selection), first, 'first snippet placeholder is selected');
    await vscode.commands.executeCommand('type', { text: 'edited' });
    await vscode.commands.executeCommand('jumpToNextSnippetPlaceholder');
    assert.equal(document.getText(editor.selection), second, 'Tab advances to the next editable field');
    report.completionRegression.snippets.push({ keyword, block, firstPlaceholder: first, nextPlaceholder: second });
    await vscode.commands.executeCommand('leaveSnippet');
  }
};
