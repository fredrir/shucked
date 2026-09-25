// Minimal stand-ins for the parts of the `vscode` API the unit tests exercise.
// Fakes record what the extension asked for so tests can assert on it.

export class Disposable {
  constructor(dispose = () => undefined) { this.dispose = dispose; }
}

export class EventEmitter {
  listeners = new Set();
  event = listener => { this.listeners.add(listener); return new Disposable(() => this.listeners.delete(listener)); };
  fire(value) { for (const listener of [...this.listeners]) { listener(value); } }
  dispose() { this.listeners.clear(); }
}

/** A configuration section backed by plain objects for each scope. */
export function configuration({ values = {}, global = {}, defaults = {} } = {}) {
  return {
    get: (key, fallback) => values[key] ?? fallback,
    inspect: key => ({ globalValue: global[key], defaultValue: defaults[key], workspaceValue: values[key] }),
    update: async () => undefined,
  };
}

/**
 * A fake API with recording window, commands, and workspace members.
 * `messages` answers information/error prompts in order.
 */
export function fakeVscode({ trusted = true, config = configuration(), messages = [] } = {}) {
  const calls = [];
  const statusItems = [];
  const registered = new Map();
  const replies = [...messages];
  const record = (name, ...args) => calls.push([name, ...args]);
  const vscode = {
    calls, statusItems, registered,
    StatusBarAlignment: { Left: 1, Right: 2 },
    EventEmitter,
    Disposable,
    workspace: {
      isTrusted: trusted,
      getConfiguration: (section, scope) => { record("getConfiguration", section, scope); return config; },
      onDidChangeConfiguration: listener => { vscode.configurationListener = listener; return new Disposable(); },
    },
    window: {
      createStatusBarItem: (alignment, priority) => {
        const item = { alignment, priority, text: "", tooltip: undefined, command: undefined, visible: false, show() { this.visible = true; }, hide() { this.visible = false; }, dispose() { this.disposed = true; } };
        statusItems.push(item);
        return item;
      },
      showInformationMessage: async (...args) => { record("showInformationMessage", ...args); return replies.shift(); },
      showErrorMessage: async (...args) => { record("showErrorMessage", ...args); return replies.shift(); },
    },
    commands: {
      registerCommand: (name, handler) => { registered.set(name, handler); return new Disposable(() => registered.delete(name)); },
      executeCommand: async (name, ...args) => { record("executeCommand", name, ...args); },
    },
  };
  return vscode;
}

/** A log output channel that remembers every line. */
export function outputChannel() {
  const lines = [];
  const log = level => message => lines.push([level, message]);
  return { lines, info: log("info"), warn: log("warn"), error: log("error"), trace: log("trace"), show: () => lines.push(["show"]) };
}

/** An extension context whose subscriptions can be inspected. */
export function extensionContext(extensionPath = "/extension") {
  return { extensionPath, subscriptions: [] };
}
