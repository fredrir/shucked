'use strict';
// Test-only bridge loaded with --extensionTestsPath. It exposes the VS Code
// extension API to pytest over an authenticated loopback socket and keeps the
// test host alive until pytest asks it to shut down. It is never packaged.
const vscode = require('vscode');
const net = require('node:net');
const fs = require('node:fs');
const crypto = require('node:crypto');

const MAX_LINE = 16 * 1024 * 1024;

function decode(value) {
  if (Array.isArray(value)) { return value.map(decode); }
  if (!value || typeof value !== 'object') { return value; }
  if (typeof value.$uri === 'string') { return vscode.Uri.parse(value.$uri); }
  if (Array.isArray(value.$position)) { return new vscode.Position(...value.$position); }
  if (Array.isArray(value.$range)) { return new vscode.Range(...value.$range); }
  if (Array.isArray(value.$selection)) { return new vscode.Selection(...value.$selection); }
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decode(item)]));
}

// Private API fields use a leading underscore and are published through getters.
function publicKeys(value) {
  const keys = new Set(Object.keys(value).filter(key => !key.startsWith('_')));
  for (let proto = Object.getPrototypeOf(value); proto && proto !== Object.prototype; proto = Object.getPrototypeOf(proto)) {
    for (const [key, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(proto))) {
      if (descriptor.get && !key.startsWith('_')) { keys.add(key); }
    }
  }
  return keys;
}

function encode(value, ancestors = new Set(), depth = 0) {
  if (value === null || value === undefined) { return null; }
  if (typeof value === 'function' || typeof value === 'symbol') { return undefined; }
  if (typeof value === 'bigint') { return Number(value); }
  if (typeof value !== 'object') { return value; }
  if (depth > 16 || ancestors.has(value)) { return null; }
  if (value instanceof vscode.Uri) { return value.toString(); }
  if (value instanceof vscode.Position) { return { line: value.line, character: value.character }; }
  if (value instanceof vscode.Range) { return { start: encode(value.start), end: encode(value.end) }; }
  if (value instanceof vscode.MarkdownString) { return value.value; }
  if (value instanceof vscode.SnippetString) { return { snippet: value.value }; }
  if (value instanceof vscode.CodeActionKind) { return value.value; }
  if (ArrayBuffer.isView(value)) { return Array.from(value); }
  if (value instanceof Error) { return { name: value.name, message: value.message }; }
  ancestors.add(value);
  try {
    if (value instanceof vscode.WorkspaceEdit) {
      return value.entries().map(([uri, edits]) => ({ uri: uri.toString(), edits: encode(edits, ancestors, depth + 1) }));
    }
    if (value instanceof Map) {
      return Object.fromEntries([...value].map(([key, item]) => [String(key), encode(item, ancestors, depth + 1)]));
    }
    if (value instanceof Set || Array.isArray(value)) {
      return [...value].map(item => encode(item, ancestors, depth + 1) ?? null);
    }
    const result = {};
    for (const key of publicKeys(value)) {
      let item;
      try { item = value[key]; } catch { continue; }
      const encoded = encode(item, ancestors, depth + 1);
      if (encoded !== undefined) { result[key] = encoded; }
    }
    return result;
  } finally { ancestors.delete(value); }
}

function documentFor(uri) {
  const key = vscode.Uri.parse(uri).toString();
  const document = vscode.workspace.textDocuments.find(item => item.uri.toString() === key);
  if (!document) { throw new Error(`Document is not open: ${uri}`); }
  return document;
}

function documentInfo(document) {
  return {
    uri: document.uri.toString(), fileName: document.fileName, languageId: document.languageId,
    version: document.version, lineCount: document.lineCount, isDirty: document.isDirty,
    isUntitled: document.isUntitled, text: document.getText(),
  };
}

function editorFor(uri) {
  const editors = vscode.window.visibleTextEditors;
  const editor = uri ? editors.find(item => item.document.uri.toString() === vscode.Uri.parse(uri).toString()) : vscode.window.activeTextEditor;
  if (!editor) { throw new Error(`No visible editor for ${uri ?? 'the active document'}`); }
  return editor;
}

// An open document scope also carries its language, so language overrides apply.
function scopeFor(scope) {
  if (!scope) { return undefined; }
  const uri = vscode.Uri.parse(scope);
  return vscode.workspace.textDocuments.find(document => document.uri.toString() === uri.toString()) ?? uri;
}

const targets = {
  global: vscode.ConfigurationTarget.Global,
  workspace: vscode.ConfigurationTarget.Workspace,
  workspaceFolder: vscode.ConfigurationTarget.WorkspaceFolder,
};

const methods = {
  ping: () => ({
    version: vscode.version, appName: vscode.env.appName, platform: `${process.platform}-${process.arch}`,
    isTrusted: vscode.workspace.isTrusted, remoteName: vscode.env.remoteName ?? null,
    workspaceFolders: (vscode.workspace.workspaceFolders ?? []).map(folder => ({ name: folder.name, uri: folder.uri.toString(), path: folder.uri.fsPath })),
    environment: { HOME: process.env.HOME, ZDOTDIR: process.env.ZDOTDIR ?? null, XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME ?? null },
  }),
  executeCommand: ({ command, args = [] }) => vscode.commands.executeCommand(command, ...decode(args)),
  // For commands that wait on a notification or dialog the test answers later.
  startCommand: ({ command, args = [] }) => {
    void Promise.resolve(vscode.commands.executeCommand(command, ...decode(args))).catch(error => console.error(`[bridge] ${command} failed`, error));
    return null;
  },
  commands: async ({ filterInternal = false }) => vscode.commands.getCommands(filterInternal),
  extension: ({ id }) => {
    const extension = vscode.extensions.getExtension(id);
    return extension ? { id: extension.id, isActive: extension.isActive, extensionPath: extension.extensionPath, version: extension.packageJSON.version } : null;
  },
  activateExtension: async ({ id }) => {
    const extension = vscode.extensions.getExtension(id);
    if (!extension) { throw new Error(`Extension is not installed: ${id}`); }
    await extension.activate();
    return methods.extension({ id });
  },
  openDocument: async ({ uri, language, content }) => {
    const document = uri
      ? await vscode.workspace.openTextDocument(vscode.Uri.parse(uri))
      : await vscode.workspace.openTextDocument({ language, content });
    return documentInfo(document);
  },
  showDocument: async ({ uri, preview = false, selection }) => {
    const document = documentFor(uri);
    const editor = await vscode.window.showTextDocument(document, { preview, selection: selection ? decode(selection) : undefined });
    return { uri: editor.document.uri.toString(), selection: encode(editor.selection) };
  },
  document: ({ uri }) => documentInfo(documentFor(uri)),
  replaceText: async ({ uri, text }) => {
    const document = documentFor(uri);
    const edit = new vscode.WorkspaceEdit();
    edit.replace(document.uri, new vscode.Range(0, 0, document.lineCount, 0), text);
    return vscode.workspace.applyEdit(edit);
  },
  applyEdit: async ({ uri, range, text }) => {
    const edit = new vscode.WorkspaceEdit();
    edit.replace(documentFor(uri).uri, new vscode.Range(...range), text);
    return vscode.workspace.applyEdit(edit);
  },
  setSelection: ({ uri, anchor, active }) => {
    const editor = editorFor(uri);
    editor.selection = new vscode.Selection(anchor[0], anchor[1], (active ?? anchor)[0], (active ?? anchor)[1]);
    return encode(editor.selection);
  },
  // Revert unsaved changes first so closing never raises a save prompt.
  resetEditors: async () => {
    for (const document of vscode.workspace.textDocuments.filter(item => item.isDirty)) {
      await vscode.window.showTextDocument(document, { preview: false });
      await vscode.commands.executeCommand('workbench.action.revertAndCloseActiveEditor');
    }
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
    return null;
  },
  setSelections: ({ uri, selections }) => {
    const editor = editorFor(uri);
    editor.selections = selections.map(([anchorLine, anchorCharacter, activeLine, activeCharacter]) => new vscode.Selection(anchorLine, anchorCharacter, activeLine, activeCharacter));
    return encode(editor.selections);
  },
  activeEditor: () => {
    const editor = vscode.window.activeTextEditor;
    return editor ? {
      uri: editor.document.uri.toString(), selection: encode(editor.selection), selections: encode(editor.selections),
      selectedText: editor.document.getText(editor.selection),
    } : null;
  },
  diagnostics: ({ uri }) => uri
    ? encode(vscode.languages.getDiagnostics(vscode.Uri.parse(uri)))
    : vscode.languages.getDiagnostics().map(([item, diagnostics]) => ({ uri: item.toString(), diagnostics: encode(diagnostics) })),
  getConfiguration: ({ section, key, scope }) => encode(vscode.workspace.getConfiguration(section, scopeFor(scope)).get(key)),
  inspectConfiguration: ({ section, key, scope }) => encode(vscode.workspace.getConfiguration(section, scopeFor(scope)).inspect(key)),
  updateConfiguration: async ({ section, key, value, target = 'global', scope }) => {
    await vscode.workspace.getConfiguration(section, scopeFor(scope)).update(key, value ?? undefined, targets[target]);
    return null;
  },
  terminals: async () => Promise.all(vscode.window.terminals.map(async terminal => ({
    name: terminal.name, processId: (await terminal.processId) ?? null, exitStatus: encode(terminal.exitStatus),
    shellIntegration: terminal.shellIntegration !== undefined,
  }))),
  terminalSendText: ({ name, text, addNewLine = true }) => {
    const terminal = vscode.window.terminals.find(item => item.name === name);
    if (!terminal) { throw new Error(`No terminal named ${name}`); }
    terminal.sendText(text, addNewLine);
    return null;
  },
  disposeTerminals: ({ names }) => {
    for (const terminal of vscode.window.terminals) { if (!names || names.includes(terminal.name)) { terminal.dispose(); } }
    return null;
  },
  // Escape hatch for API surfaces without a dedicated method; prefer the methods above.
  evaluate: ({ code, args = {} }) => {
    const AsyncFunction = Object.getPrototypeOf(async () => undefined).constructor;
    return new AsyncFunction('vscode', 'args', code)(vscode, decode(args));
  },
};

function serve(socket, token, finish) {
  let buffer = '';
  let authenticated = false;
  socket.setEncoding('utf8');
  socket.on('error', () => socket.destroy());
  const reply = message => { if (!socket.destroyed) { socket.write(`${JSON.stringify(message)}\n`); } };
  socket.on('data', chunk => {
    buffer += chunk;
    if (buffer.length > MAX_LINE) { socket.destroy(); return; }
    let newline;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      let request;
      try { request = JSON.parse(line); } catch { socket.destroy(); return; }
      if (!authenticated) {
        const expected = Buffer.from(token), actual = Buffer.from(String(request.token ?? ''));
        if (expected.length !== actual.length || !crypto.timingSafeEqual(expected, actual)) { socket.destroy(); return; }
        authenticated = true;
        reply({ ready: true });
        continue;
      }
      if (request.method === 'shutdown') {
        reply({ id: request.id, result: null });
        socket.end();
        finish(request.params?.code ?? 0, request.params?.message);
        continue;
      }
      const handler = methods[request.method];
      Promise.resolve()
        .then(() => { if (!handler) { throw new Error(`Unknown bridge method: ${request.method}`); } return handler(request.params ?? {}); })
        .then(result => reply({ id: request.id, result: encode(result) ?? null }))
        .catch(error => reply({ id: request.id, error: { message: error?.message ?? String(error), stack: error?.stack ?? null } }));
    }
  });
}

exports.run = function run() {
  const tokenFile = process.env.SHUCKED_BRIDGE_TOKEN_FILE;
  const portFile = process.env.SHUCKED_BRIDGE_PORT_FILE;
  if (!tokenFile || !portFile) { return Promise.reject(new Error('The bridge requires SHUCKED_BRIDGE_TOKEN_FILE and SHUCKED_BRIDGE_PORT_FILE')); }
  // Read once and delete, so no later process can learn the secret.
  const token = fs.readFileSync(tokenFile, 'utf8').trim();
  fs.unlinkSync(tokenFile);
  if (!/^[a-f0-9]{64}$/.test(token)) { return Promise.reject(new Error('The bridge token is malformed')); }
  return new Promise((resolve, reject) => {
    const server = net.createServer(socket => serve(socket, token, (code, message) => {
      server.close();
      if (code === 0) { resolve(); } else { reject(new Error(message ?? `Bridge finished with status ${code}`)); }
    }));
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      // Publish atomically so pytest never reads a partially written port.
      fs.writeFileSync(`${portFile}.tmp`, String(server.address().port), { mode: 0o600 });
      fs.renameSync(`${portFile}.tmp`, portFile);
    });
  });
};
