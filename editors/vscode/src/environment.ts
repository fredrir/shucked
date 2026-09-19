import * as vscode from "vscode";
import { ClientManager } from "./client";

export interface EnvironmentSelection {
  policy?: "workspace" | "portable";
  cwd?: string;
  targetInventory?: string;
  sessionId?: string;
}

export class EnvironmentManager implements vscode.Disposable {
  private readonly status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 99);
  private readonly selections = new Map<string, EnvironmentSelection>();
  private readonly subscriptions: vscode.Disposable[] = [];
  private readonly sessions = new Map<string, { connected?: boolean; shell?: string; cwd?: string }>();

  constructor(private readonly context: vscode.ExtensionContext, private readonly client: ClientManager) {
    this.status.command = "shucked.selectEnvironment";
    this.subscriptions.push(
      this.status,
      this.client.onReady(() => { for (const document of vscode.workspace.textDocuments) { const selection = this.selection(document); if (selection) { void this.client.notify("shucked/selectEnvironment", { uri: document.uri.toString(), options: selection }); } } }),
      vscode.window.onDidChangeActiveTextEditor(() => this.updateStatus()),
      vscode.workspace.onDidChangeConfiguration(() => this.updateStatus()),
      vscode.workspace.onDidOpenTextDocument(document => { void this.restore(document); }),
      vscode.commands.registerCommand("shucked.selectEnvironment", () => this.select()),
    );
    for (const document of vscode.workspace.textDocuments) { void this.restore(document); }
    this.updateStatus();
  }

  public sessionState(id: string, connected: boolean | undefined, metadata?: { shell: string; cwd: string }): void {
    this.sessions.set(id, { connected, ...metadata });
    this.updateStatus();
  }

  public async attachSession(id: string): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (document) { await this.apply(document, { sessionId: id }); }
  }

  public selection(document: vscode.TextDocument): EnvironmentSelection | undefined {
    return this.selections.get(document.uri.toString());
  }

  private async restore(document: vscode.TextDocument): Promise<void> {
    const selection = this.context.workspaceState.get<EnvironmentSelection>(`environment:${document.uri.toString()}`);
    if (selection) { this.selections.set(document.uri.toString(), selection); await this.client.notify("shucked/selectEnvironment", { uri: document.uri.toString(), options: selection }); }
    this.updateStatus();
  }

  private async apply(document: vscode.TextDocument, selection: EnvironmentSelection | undefined): Promise<void> {
    const uri = document.uri.toString();
    if (selection) { this.selections.set(uri, selection); } else { this.selections.delete(uri); }
    // A live process cannot be restored across extension sessions.
    await this.context.workspaceState.update(`environment:${uri}`, selection?.sessionId ? undefined : selection);
    await this.client.notify("shucked/selectEnvironment", { uri, options: selection ?? null });
    this.updateStatus();
  }

  private async select(): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (!document) { return; }
    const choice = await vscode.window.showQuickPick([
      { label: "Workspace host", description: `${vscode.env.remoteName ?? "local"} · script semantics`, id: "workspace" },
      { label: "Portable", description: "Syntax and source checks; no host absence warnings", id: "portable" },
      { label: "Captured target…", description: "Use an offline target inventory", id: "captured" },
      { label: "Set launch directory…", description: "Make relative command and path checks explicit", id: "cwd" },
      { label: "Use workspace settings", description: "Remove this document's override", id: "reset" },
    ], { title: "Shucked execution context" });
    if (!choice) { return; }
    if (choice.id === "captured") {
      const picked = await vscode.window.showOpenDialog({ canSelectMany: false, filters: { "Target inventory": ["json"] }, title: "Select captured target" });
      if (picked?.[0]) { await this.apply(document, { targetInventory: picked[0].fsPath }); }
    } else if (choice.id === "cwd") {
      const picked = await vscode.window.showOpenDialog({ canSelectMany: false, canSelectFiles: false, canSelectFolders: true, title: "Script launch directory" });
      if (picked?.[0]) { await this.apply(document, { ...this.selection(document), cwd: picked[0].fsPath }); }
    } else { await this.apply(document, choice.id === "reset" ? undefined : { policy: choice.id as "workspace" | "portable" }); }
  }

  private updateStatus(): void {
    const document = vscode.window.activeTextEditor?.document;
    if (!document || !["shellscript", "bash", "zsh", "fish", "sh", "ksh"].includes(document.languageId)) { this.status.hide(); return; }
    const selection = this.selection(document) ?? vscode.workspace.getConfiguration("shucked", document).get<EnvironmentSelection>("environment", {});
    const session = selection.sessionId ? this.sessions.get(selection.sessionId) : undefined;
    const target = selection.sessionId ? session?.connected === true ? `Terminal (${session.shell ?? "shell"})` : session?.connected === false ? "Terminal disconnected" : "Terminal pending" : selection.targetInventory ? "Captured" : selection.policy === "portable" ? "Portable" : `Workspace (${vscode.env.remoteName ?? "local"})`;
    const startup = /(?:^|[/\\])(?:\.zshrc|\.zshenv|\.zprofile|\.bashrc|\.bash_profile|\.profile|config\.fish)$/.test(document.uri.path);
    this.status.text = `$(terminal) ${target}`;
    this.status.tooltip = `Shucked · ${document.languageId} · ${startup ? "startup file" : selection.sessionId ? "interactive session" : "script"}\nLaunch directory: ${launchDirectoryLabel(startup, selection, session?.cwd)}\nSelect execution context`;
    this.status.show();
  }

  public dispose(): void { for (const subscription of this.subscriptions) { subscription.dispose(); } }
}

export function launchDirectoryLabel(startup: boolean, selection: EnvironmentSelection, sessionCwd?: string): string {
  if (startup && selection.sessionId) { return "unknown at startup (terminal state was captured later)"; }
  return sessionCwd || selection.cwd || "assumed workspace folder";
}
