import * as path from "node:path";
import * as vscode from "vscode";
import { ClientManager } from "./client";

export type EnvironmentPolicy = "workspace" | "portable" | "login-shell";
export interface EnvironmentSelection {
  policy?: EnvironmentPolicy;
  workspaceUri?: string;
  cwd?: string;
  targetInventory?: string;
  sessionId?: string;
  /** Login shell executable for the `login-shell` policy; `$SHELL` otherwise. */
  loginShell?: string;
}
interface SessionState { connected?: boolean; shell?: string; cwd?: string; problem?: string }
interface EnvironmentDetailsResult { markdown: string; trusted: boolean; source: string }

/** The shell name the server will capture for the login-shell policy. */
export function loginShellName(selection: EnvironmentSelection, env: NodeJS.ProcessEnv = process.env, platform: NodeJS.Platform = process.platform): string {
  const configured = selection.loginShell || env.SHELL;
  return configured ? path.basename(configured) : platform === "darwin" ? "zsh" : "bash";
}

/** Status bar label for the selected execution context. */
export function contextLabel(selection: EnvironmentSelection, session: SessionState | undefined, remoteName: string | undefined, loginShell: string): string {
  if (selection.sessionId) {
    if (session?.connected === true) { return `Terminal (${session.shell ?? "shell"})`; }
    if (session?.connected === false) { return "Terminal disconnected"; }
    return session?.problem ? `Terminal unavailable (${session.problem})` : "Terminal pending";
  }
  if (selection.targetInventory) { return "Captured"; }
  if (selection.policy === "portable") { return "Portable"; }
  if (selection.policy === "login-shell") { return `Login shell (${loginShell})`; }
  return `Workspace (${remoteName ?? "local"})`;
}

export class EnvironmentManager implements vscode.Disposable {
  private readonly status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 99);
  private readonly selections = new Map<string, EnvironmentSelection>();
  private readonly subscriptions: vscode.Disposable[] = [];
  private readonly sessions = new Map<string, SessionState>();

  constructor(private readonly context: vscode.ExtensionContext, private readonly client: ClientManager) {
    this.status.command = "shucked.selectEnvironment";
    this.subscriptions.push(
      this.status,
      this.client.onReady(() => { for (const document of vscode.workspace.textDocuments) { const selection = this.selection(document); if (selection) { void this.client.notify("shucked/selectEnvironment", { uri: document.uri.toString(), options: selection }); } } }),
      vscode.window.onDidChangeActiveTextEditor(() => this.updateStatus()),
      vscode.workspace.onDidChangeConfiguration(() => this.updateStatus()),
      vscode.workspace.onDidOpenTextDocument(document => { void this.restore(document); }),
      vscode.commands.registerCommand("shucked.selectEnvironment", () => this.select()),
      vscode.commands.registerCommand("shucked.showEnvironmentDetails", () => this.showDetails()),
    );
    for (const document of vscode.workspace.textDocuments) { void this.restore(document); }
    this.updateStatus();
  }

  public sessionState(id: string, connected: boolean | undefined, metadata?: { shell: string; cwd: string }, problem?: string): void {
    this.sessions.set(id, { connected, ...metadata, problem });
    this.updateStatus();
  }

  public async attachSession(id: string): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (document) { await this.apply(document, { workspaceUri: this.selection(document)?.workspaceUri, sessionId: id }); }
  }

  public selection(document: vscode.TextDocument): EnvironmentSelection | undefined {
    return this.selections.get(document.uri.toString());
  }

  /** The context in effect: the document override, otherwise the `shucked.environment` settings. */
  public effectiveSelection(document: vscode.TextDocument): EnvironmentSelection {
    return this.selection(document) ?? vscode.workspace.getConfiguration("shucked", document).get<EnvironmentSelection>("environment", {});
  }

  private async restore(document: vscode.TextDocument): Promise<void> {
    if (document.isUntitled) { this.updateStatus(); return; }
    const selection = this.context.workspaceState.get<EnvironmentSelection>(`environment:${document.uri.toString()}`);
    if (selection) { this.selections.set(document.uri.toString(), selection); await this.client.notify("shucked/selectEnvironment", { uri: document.uri.toString(), options: selection }); }
    this.updateStatus();
  }

  private async apply(document: vscode.TextDocument, selection: EnvironmentSelection | undefined): Promise<void> {
    const uri = document.uri.toString();
    void vscode.commands.executeCommand("editor.action.inlineSuggest.hide");
    if (selection) { this.selections.set(uri, selection); } else { this.selections.delete(uri); }
    // A live process cannot be restored across extension sessions.
    await this.context.workspaceState.update(`environment:${uri}`, selection?.sessionId || document.isUntitled ? undefined : selection);
    await this.client.notify("shucked/selectEnvironment", { uri, options: selection ?? null });
    if (selection?.policy === "login-shell" && !vscode.workspace.isTrusted) {
      void vscode.window.showInformationMessage("Login shell capture runs your startup files and requires workspace trust. The workspace host environment stays in use until the workspace is trusted.");
    }
    this.updateStatus();
  }

  private async select(): Promise<void> {
    const document = vscode.window.activeTextEditor?.document;
    if (!document) { return; }
    const loginShell = loginShellName(this.effectiveSelection(document));
    const choice = await vscode.window.showQuickPick([
      ...(document.isUntitled ? [{ label: "Associate workspace…", description: "Select this buffer's settings and assumed launch directory", id: "association" }] : []),
      { label: "Workspace host", description: `${vscode.env.remoteName ?? "local"} · script semantics`, id: "workspace" },
      { label: "Login shell", description: `${loginShell} startup files · aliases, functions and PATH${vscode.workspace.isTrusted ? "" : " · requires workspace trust"}`, id: "login-shell" },
      { label: "Portable", description: "Syntax and source checks; no host absence warnings", id: "portable" },
      { label: "Captured target…", description: "Use an offline target inventory", id: "captured" },
      { label: "Set launch directory…", description: "Make relative command and path checks explicit", id: "cwd" },
      { label: "Show details", description: "Trust, evidence source, PATH, providers and the command at the cursor", id: "details" },
      { label: "Use workspace settings", description: "Remove this document's override", id: "reset" },
    ], { title: "Shucked execution context" });
    if (!choice) { return; }
    const association = { workspaceUri: this.selection(document)?.workspaceUri };
    if (choice.id === "association") {
      const folders = vscode.workspace.workspaceFolders ?? [];
      const folder = await vscode.window.showQuickPick(folders.map(folder => ({ label: folder.name, description: folder.uri.fsPath, folder })), { title: "Workspace for untitled shell buffer" });
      if (folder) { await this.apply(document, { ...this.selection(document), workspaceUri: folder.folder.uri.toString() }); }
    } else if (choice.id === "captured") {
      const picked = await vscode.window.showOpenDialog({ canSelectMany: false, filters: { "Target inventory": ["json"] }, title: "Select captured target" });
      if (picked?.[0]) { await this.apply(document, { ...association, targetInventory: picked[0].fsPath }); }
    } else if (choice.id === "cwd") {
      const picked = await vscode.window.showOpenDialog({ canSelectMany: false, canSelectFiles: false, canSelectFolders: true, title: "Script launch directory" });
      if (picked?.[0]) { await this.apply(document, { ...this.selection(document), cwd: picked[0].fsPath }); }
    } else if (choice.id === "details") {
      await this.showDetails();
    } else { await this.apply(document, choice.id === "reset" ? undefined : { ...association, policy: choice.id as EnvironmentPolicy }); }
  }

  /** Open a Markdown report of the execution context for the active document. */
  public async showDetails(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) { void vscode.window.showInformationMessage("Open a shell script to inspect its execution context."); return; }
    const document = editor.document;
    const result = await this.client.request<EnvironmentDetailsResult>("shucked/environmentDetails", {
      textDocument: { uri: document.uri.toString() },
      position: { line: editor.selection.active.line, character: editor.selection.active.character },
    }).catch((error: unknown) => { void vscode.window.showErrorMessage(`Shucked could not describe the environment: ${error instanceof Error ? error.message : String(error)}`); return undefined; });
    if (!result) { if (!this.client.isRunning) { void vscode.window.showInformationMessage("The Shucked language server is not running."); } return; }
    const selection = this.selection(document);
    const session = selection?.sessionId ? this.sessions.get(selection.sessionId) : undefined;
    const client = [
      "## Editor",
      "",
      `- Workspace trust: ${vscode.workspace.isTrusted ? "trusted" : "untrusted"} (server: ${result.trusted ? "native execution allowed" : "native execution denied"})`,
      `- Host: ${vscode.env.remoteName ?? "local"} · ${process.platform}/${process.arch}`,
      `- Document override: ${selection ? `\`${JSON.stringify(selection)}\`` : "none (workspace settings apply)"}`,
      `- Settings: \`${JSON.stringify(vscode.workspace.getConfiguration("shucked", document).get("environment", {}))}\``,
      `- Login shell (client view): ${loginShellName(this.effectiveSelection(document))}`,
      ...(session ? [`- Terminal session: ${contextLabel(selection ?? {}, session, vscode.env.remoteName, "")}${session.cwd ? ` · cwd ${session.cwd}` : ""}`] : []),
      "",
    ].join("\n");
    const report = await vscode.workspace.openTextDocument({ language: "markdown", content: `${result.markdown}\n${client}` });
    await vscode.window.showTextDocument(report, { preview: true, viewColumn: vscode.ViewColumn.Beside, preserveFocus: false });
  }

  private updateStatus(): void {
    const document = vscode.window.activeTextEditor?.document;
    if (!document || !["shellscript", "bash", "zsh", "fish", "sh", "ksh"].includes(document.languageId)) { this.status.hide(); return; }
    const selection = this.effectiveSelection(document);
    const associatedFolder = selection.workspaceUri ? vscode.workspace.workspaceFolders?.find(folder => folder.uri.toString() === selection.workspaceUri) : undefined;
    const assumedFolder = associatedFolder ?? (!selection.workspaceUri && document.isUntitled && vscode.workspace.workspaceFolders?.length === 1 ? vscode.workspace.workspaceFolders[0] : vscode.workspace.getWorkspaceFolder(document.uri));
    const session = selection.sessionId ? this.sessions.get(selection.sessionId) : undefined;
    const target = contextLabel(selection, session, vscode.env.remoteName, loginShellName(selection));
    const startup = /(?:^|[/\\])(?:\.zshrc|\.zshenv|\.zprofile|\.bashrc|\.bash_profile|\.profile|config\.fish)$/.test(document.uri.path);
    this.status.text = `$(terminal) ${target}${document.isUntitled ? ` · ${assumedFolder?.name ?? "select workspace"}` : ""}`;
    this.status.tooltip = `Shucked · ${document.languageId} · ${startup ? "startup file" : selection.sessionId ? "interactive session" : selection.policy === "login-shell" ? "login shell context" : "script"}\nLaunch directory: ${launchDirectoryLabel(startup, selection, session?.cwd, assumedFolder?.uri.fsPath)}\nSelect execution context`;
    this.status.show();
  }

  public dispose(): void { for (const subscription of this.subscriptions) { subscription.dispose(); } }
}

export function launchDirectoryLabel(startup: boolean, selection: EnvironmentSelection, sessionCwd?: string, assumedCwd?: string): string {
  if (startup && selection.sessionId) { return "unknown at startup (terminal state was captured later)"; }
  return sessionCwd || selection.cwd || (assumedCwd ? `${assumedCwd} (assumed)` : "unknown workspace");
}
