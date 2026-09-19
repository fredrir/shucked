import * as vscode from "vscode";
import * as fs from "node:fs/promises";
import { constants } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { createHash } from "node:crypto";
import { EnvironmentManager, type EnvironmentSelection } from "./environment";
import type { SessionMetadata } from "./terminal";

const MAX_ENTRIES = 1000;
const MAX_COMMAND = 8192;
const MAX_FILE = 2 * 1024 * 1024;
const languages = ["shellscript", "bash", "zsh", "fish", "sh", "ksh"];

/** Bounded context-local history; never persists commands. */
export class HistoryIndex {
  private readonly entries = new Map<string, string[]>();
  public record(context: string, command: string): void {
    if (!command || command.length > MAX_COMMAND || command.startsWith(" ") || /[\r\n\0]/.test(command)) { return; }
    const entries = this.entries.get(context) ?? [];
    const previous = entries.indexOf(command); if (previous >= 0) { entries.splice(previous, 1); }
    entries.push(command); if (entries.length > MAX_ENTRIES) { entries.splice(0, entries.length - MAX_ENTRIES); }
    this.entries.set(context, entries);
    while (this.entries.size > 32) { const first = this.entries.keys().next().value; if (first) { this.entries.delete(first); } }
  }
  public suggest(context: string, prefix: string): string | undefined {
    if (prefix.length < 2) { return undefined; }
    return this.entries.get(context)?.findLast(command => command.startsWith(prefix) && command !== prefix);
  }
  public clear(context?: string): void { if (context) { this.entries.delete(context); } else { this.entries.clear(); } }
}

/** Session suggestions require a fresh prompt confirming shell history acceptance. */
export function acceptedSessionCommand(text: string, metadata: Pick<SessionMetadata, "private" | "ignore" | "acceptedHistoryHash">): boolean {
  return !metadata.private && metadata.ignore.every(rule => rule === "leading-space")
    && !text.startsWith(" ") && !/[\r\n\0]/.test(text)
    && metadata.acceptedHistoryHash === createHash("sha256").update(text.trim()).digest("hex");
}

/** Read shell formats as data, with no sourcing or shell evaluation. */
export function parseHistory(text: string, shell: string): string[] {
  const result: string[] = [];
  if (shell === "fish") {
    for (const line of text.split("\n")) {
      if (line.startsWith("- cmd: ")) {
        const value = line.slice(7).replace(/\\([\\n])/g, (_, character: string) => character === "n" ? "\n" : "\\");
        if (!value.includes("\n")) { result.push(value); }
      }
    }
  } else {
    let continuation = false;
    for (const line of text.split("\n")) {
      const value = shell === "zsh" ? line.replace(/^: \d+:\d+;/, "") : line;
      const skip = continuation; continuation = /(^|[^\\])(\\\\)*\\$/.test(value);
      if (!skip && !continuation && value && !/^#\d+$/.test(value)) { result.push(value); }
    }
  }
  return result.slice(-MAX_ENTRIES);
}

export class HistoryManager implements vscode.Disposable {
  private readonly index = new HistoryIndex();
  private readonly subscriptions: vscode.Disposable[] = [];
  private readonly fileReads = new Map<string, number>();
  private generation = 0;
  private readonly sessions = new Map<string, SessionMetadata>();
  constructor(private readonly environments: EnvironmentManager) {
    this.subscriptions.push(
      vscode.workspace.onDidChangeConfiguration(event => { if (event.affectsConfiguration("shucked.history")) { this.generation += 1; this.index.clear(); this.fileReads.clear(); void vscode.commands.executeCommand("editor.action.inlineSuggest.hide"); } }),
      vscode.languages.registerInlineCompletionItemProvider(languages.map(language => ({ language, scheme: "file" })), { provideInlineCompletionItems: (document, position, _context, cancellation) => this.provide(document, position, cancellation) }),
      vscode.commands.registerCommand("shucked.clearHistorySuggestions", () => { this.generation += 1; this.index.clear(); this.fileReads.clear(); void vscode.commands.executeCommand("editor.action.inlineSuggest.hide"); }),
    );
  }
  public sessionEnabled(): boolean { return vscode.workspace.isTrusted && vscode.workspace.getConfiguration("shucked").get<boolean>("history.session", false); }
  public filesEnabled(): boolean { return vscode.workspace.isTrusted && vscode.workspace.getConfiguration("shucked").get<boolean>("history.files", false); }
  public updateSession(metadata: SessionMetadata): void {
    const previous = this.sessions.get(metadata.id);
    this.sessions.set(metadata.id, metadata);
    if (previous?.historyFile !== metadata.historyFile || previous?.private !== metadata.private) {
      this.generation += 1;
      this.index.clear(`files:${metadata.id}`); this.fileReads.delete(`files:${metadata.id}`);
      if (metadata.private) { this.index.clear(`session:${metadata.id}`); void vscode.commands.executeCommand("editor.action.inlineSuggest.hide"); }
    }
  }
  public recordSession(id: string, text: string, metadata: SessionMetadata): void {
    if (!this.sessionEnabled() || !acceptedSessionCommand(text, metadata)) { return; }
    // Until a prompt updates state, nested shells/remotes cannot establish host-local history.
    if (/^\s*(?:ssh|mosh|su|sudo|docker|podman|bash|zsh|fish)(?:\s|$)/.test(text)) { return; }
    this.index.record(`session:${id}`, text);
  }
  public clearSession(id: string): void { this.sessions.delete(id); this.index.clear(`session:${id}`); this.index.clear(`files:${id}`); this.fileReads.delete(`files:${id}`); this.generation += 1; void vscode.commands.executeCommand("editor.action.inlineSuggest.hide"); }

  private async provide(document: vscode.TextDocument, position: vscode.Position, cancellation: vscode.CancellationToken): Promise<vscode.InlineCompletionItem[]> {
    if (!vscode.workspace.isTrusted || !languages.includes(document.languageId)) { return []; }
    const config = vscode.workspace.getConfiguration("shucked", document);
    const sessionEnabled = config.get<boolean>("history.session", false);
    const filesEnabled = config.get<boolean>("history.files", false);
    if (!sessionEnabled && !filesEnabled) { return []; }
    const selection = this.environments.selection(document) ?? config.get<EnvironmentSelection>("environment", {});
    const session = selection?.sessionId ? this.sessions.get(selection.sessionId) : undefined;
    if (selection?.sessionId && (!session || session.private)) { return []; }
    const documentVersion = document.version;
    const selectionKey = JSON.stringify(selection);
    if (selection?.targetInventory || selection?.policy === "portable") { return []; }
    const line = document.lineAt(position.line).text;
    if (position.character !== line.length) { return []; }
    const prefix = line.trimStart();
    if (prefix.length < 2 || prefix.startsWith("#")) { return []; }
    const shell = document.languageId === "fish" || document.fileName.endsWith(".fish") ? "fish" : document.languageId === "zsh" || document.fileName.endsWith(".zsh") || document.fileName.endsWith(".zshrc") ? "zsh" : "bash";
    const context = session ? `files:${session.id}` : `workspace:${vscode.workspace.getWorkspaceFolder(document.uri)?.uri.toString() ?? document.uri.toString()}:${shell}`;
    const generation = this.generation;
    if (filesEnabled) { await this.readHistory(context, shell, session?.historyFile); }
    const currentSelection = this.environments.selection(document) ?? vscode.workspace.getConfiguration("shucked", document).get<EnvironmentSelection>("environment", {});
    if (cancellation.isCancellationRequested || generation !== this.generation || document.version !== documentVersion || JSON.stringify(currentSelection) !== selectionKey) { return []; }
    const suggestion = sessionEnabled && selection?.sessionId ? this.index.suggest(`session:${selection.sessionId}`, prefix) : undefined;
    const candidate = suggestion ?? (filesEnabled ? this.index.suggest(context, prefix) : undefined);
    if (!candidate) { return []; }
    return [new vscode.InlineCompletionItem(candidate.slice(prefix.length), new vscode.Range(position, position))];
  }

  private async readHistory(context: string, shell: string, selectedFile?: string): Promise<void> {
    // This method is called only after the independent existing-file opt-in.
    const previous = this.fileReads.get(context) ?? 0;
    if (Date.now() - previous < 30000) { return; }
    this.fileReads.set(context, Date.now());
    const generation = this.generation;
    if (selectedFile === "") { return; }
    const filename = selectedFile ?? (shell === "fish" ? path.join(process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local", "share"), "fish", "fish_history") : path.join(os.homedir(), shell === "zsh" ? ".zsh_history" : ".bash_history"));
    try {
      const text = await readHistoryFile(filename);
      if (text === undefined || generation !== this.generation || !vscode.workspace.getConfiguration("shucked").get<boolean>("history.files", false)) { return; }
      for (const command of parseHistory(text, shell)) { this.index.record(context, command); }
    } catch { /* Missing/private history remains unavailable, without content logging. */ }
  }
  public dispose(): void { this.sessions.clear(); this.generation += 1; this.index.clear(); this.fileReads.clear(); for (const subscription of this.subscriptions) { subscription.dispose(); } }
}

/** Inspect only a regular history file; FIFOs must not occupy an I/O worker. */
export async function readHistoryFile(filename: string): Promise<string | undefined> {
  const file = await fs.open(filename, constants.O_RDONLY | (process.platform === "win32" ? 0 : constants.O_NONBLOCK));
  try {
    const stat = await file.stat();
    if (!stat.isFile()) { return undefined; }
    const size = Math.min(stat.size, MAX_FILE);
    const buffer = Buffer.alloc(size);
    const { bytesRead } = await file.read(buffer, 0, size, Math.max(0, stat.size - size));
    let text = buffer.subarray(0, bytesRead).toString("utf8");
    if (stat.size > MAX_FILE) { text = text.slice(text.indexOf("\n") + 1); }
    return text;
  } finally { await file.close(); }
}
