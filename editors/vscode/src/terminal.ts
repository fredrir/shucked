import * as vscode from "vscode";
import * as net from "node:net";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { ClientManager } from "./client";
import { EnvironmentManager } from "./environment";
import type { HistoryManager } from "./history";

type Shell = "bash" | "zsh" | "fish";
export interface SessionMetadata {
  id: string; generation: number; pid: number; shell: Shell; cwd: string; path: string[];
  aliases: Record<string, string[]>; functions: string[]; options: Record<string, string>;
  private: boolean; ignore: string[]; connected: boolean;
}
interface AttachedSession { id: string; token: string; shell: Shell; generation: number; terminal?: vscode.Terminal; metadata?: SessionMetadata; pid?: number; }
const MAX_FRAME = 256 * 1024;
const stringList = (value: unknown, max = 16384): value is string[] => Array.isArray(value) && value.length <= max && value.every(item => typeof item === "string" && item.length < 16384 && !item.includes("\0"));

/** Validate untrusted local IPC before publishing any shell metadata. */
export function validateSessionMessage(value: unknown): value is SessionMetadata & { token: string } {
  if (!value || typeof value !== "object") { return false; }
  const item = value as Record<string, unknown>;
  if (typeof item.id !== "string" || !/^[a-f0-9]{32}$/.test(item.id) || typeof item.token !== "string" || !/^[a-f0-9]{64}$/.test(item.token)) { return false; }
  if (!Number.isSafeInteger(item.generation) || Number(item.generation) < 1 || !Number.isSafeInteger(item.pid) || Number(item.pid) <= 0) { return false; }
  if (!["bash", "zsh", "fish"].includes(String(item.shell)) || typeof item.cwd !== "string" || !path.isAbsolute(item.cwd) || item.cwd.includes("\0")) { return false; }
  if (!stringList(item.path, 1024) || !stringList(item.functions) || !stringList(item.ignore, 32)) { return false; }
  if (typeof item.private !== "boolean" || item.connected !== true || !item.aliases || typeof item.aliases !== "object" || Array.isArray(item.aliases)) { return false; }
  if (Object.keys(item.aliases).length > 16384 || !Object.entries(item.aliases).every(([name, words]) => /^[\w.:-]{1,256}$/.test(name) && stringList(words, 32))) { return false; }
  return !!item.options && typeof item.options === "object" && !Array.isArray(item.options) && Object.entries(item.options).length < 128 && Object.entries(item.options).every(([key, value]) => /^[\w_-]+$/.test(key) && typeof value === "string" && value.length < 128);
}

export class TerminalManager implements vscode.Disposable {
  private readonly sessions = new Map<string, AttachedSession>();
  private readonly subscriptions: vscode.Disposable[] = [];
  private server?: net.Server;
  private directory?: string;
  private socketPath?: string;
  private starting?: Promise<void>;
  private disposed = false;

  constructor(private readonly context: vscode.ExtensionContext, private readonly client: ClientManager, private readonly environments: EnvironmentManager, private readonly history: HistoryManager) {
    this.subscriptions.push(
      vscode.commands.registerCommand("shucked.createTerminal", () => this.create()),
      vscode.commands.registerCommand("shucked.attachTerminal", () => this.attach()),
      vscode.window.registerTerminalProfileProvider("shucked.terminal", { provideTerminalProfile: async () => {
        if (!vscode.workspace.isTrusted) { throw new Error("Shucked terminals require workspace trust"); }
        const shell = await this.selectShell(); if (!shell) { return undefined; }
        const { session, options } = await this.prepare(shell);
        await this.environments.attachSession(session.id);
        return new vscode.TerminalProfile(options);
      } }),
      vscode.window.onDidOpenTerminal(terminal => {
        const options = terminal.creationOptions;
        if ("env" in options && typeof options.env?.SHUCKED_SESSION_ID === "string") { this.link(terminal, options.env.SHUCKED_SESSION_ID); }
      }),
      vscode.window.onDidCloseTerminal(terminal => { for (const session of this.sessions.values()) { if (session.terminal === terminal) { this.disconnect(session); } } }),
      vscode.window.onDidEndTerminalShellExecution(event => {
        // Do not access the command text while session collection is disabled.
        if (!this.history.sessionEnabled()) { return; }
        const session = [...this.sessions.values()].find(session => session.terminal === event.terminal);
        if (!session?.metadata || !event.execution.commandLine.isTrusted) { return; }
        this.history.recordSession(session.id, event.execution.commandLine.value, session.metadata);
      }),
    );
  }

  private async ensureServer(): Promise<void> {
    if (this.starting) { return this.starting; }
    this.starting = (async () => {
      this.directory = await fs.mkdtemp(path.join(os.tmpdir(), "shucked-session-"));
      await fs.chmod(this.directory, 0o700);
      this.socketPath = process.platform === "win32" ? `\\\\.\\pipe\\shucked-${randomBytes(24).toString("hex")}` : path.join(this.directory, "state.sock");
      this.server = net.createServer(socket => {
        socket.setTimeout(1500, () => socket.destroy());
        let frame = Buffer.alloc(0); let handled = false;
        socket.on("error", () => socket.destroy());
        socket.on("data", chunk => {
          if (handled || frame.length + chunk.length > MAX_FRAME) { socket.destroy(); return; }
          frame = Buffer.concat([frame, chunk]);
          if (!frame.includes(10)) { return; }
          handled = true;
          try {
            const value: unknown = JSON.parse(frame.subarray(0, frame.indexOf(10)).toString("utf8"));
            if (validateSessionMessage(value)) { void this.receive(value).catch(() => undefined); }
          } catch { /* Malformed data is discarded without logging shell contents. */ }
          socket.end();
        });
      });
      this.server.maxConnections = 32;
      await new Promise<void>((resolve, reject) => { this.server?.once("error", reject); this.server?.listen(this.socketPath, resolve); });
      if (process.platform !== "win32") { await fs.chmod(this.socketPath, 0o600); }
    })();
    return this.starting;
  }

  private async receive(message: SessionMetadata & { token: string }): Promise<void> {
    const session = this.sessions.get(message.id);
    if (this.disposed || !vscode.workspace.isTrusted || !session || session.shell !== message.shell || message.generation <= session.generation) { return; }
    const expected = Buffer.from(session.token), actual = Buffer.from(message.token);
    if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) { return; }
    if (session.pid && session.pid !== message.pid) { return; }
    session.pid ??= message.pid;
    session.generation = message.generation;
    const { token: _token, ...metadata } = message;
    session.metadata = metadata;
    await this.client.notify("shucked/shellSession", metadata);
  }

  private async selectShell(): Promise<Shell | undefined> {
    const preferred = path.basename(process.env.SHELL ?? "bash") as Shell;
    const shells: Shell[] = ["bash", "zsh", "fish"];
    const items = shells.sort((a, b) => Number(b === preferred) - Number(a === preferred)).map(shell => ({ label: shell, description: shell === preferred ? "Workspace host login shell" : "Installed workspace host shell", shell }));
    return (await vscode.window.showQuickPick(items, { title: "Shucked terminal shell" }))?.shell;
  }

  private async prepare(shell: Shell): Promise<{ session: AttachedSession; options: vscode.TerminalOptions }> {
    await this.ensureServer();
    const session: AttachedSession = { id: randomBytes(16).toString("hex"), token: randomBytes(32).toString("hex"), shell, generation: 0 };
    this.sessions.set(session.id, session);
    const integration = path.join(this.context.extensionPath, "shell-integration");
    const hook = path.join(integration, shell === "zsh" ? "zsh.zsh" : shell === "fish" ? "fish.fish" : "bash.sh");
    const env: Record<string, string> = { SHUCKED_SESSION_ID: session.id, SHUCKED_SESSION_TOKEN: session.token, SHUCKED_SESSION_SOCKET: this.socketPath!, SHUCKED_NODE: process.execPath, SHUCKED_CAPTURE: path.join(integration, "capture.cjs") };
    const sessionDirectory = path.join(this.directory!, session.id); await fs.mkdir(sessionDirectory, { mode: 0o700 });
    let shellArgs: string[];
    if (shell === "bash") {
      const init = path.join(sessionDirectory, "bashrc");
      await fs.writeFile(init, `[[ -f ~/.bashrc ]] && source ~/.bashrc\nsource ${quote(hook)}\n`, { mode: 0o600 });
      shellArgs = ["--rcfile", init, "-i"];
    } else if (shell === "zsh") {
      const original = process.env.ZDOTDIR ?? os.homedir();
      await fs.writeFile(path.join(sessionDirectory, ".zshenv"), `ZDOTDIR=${quote(original)}\n[[ -r $ZDOTDIR/.zshenv ]] && source $ZDOTDIR/.zshenv\n__shucked_original_zdotdir=$ZDOTDIR\nZDOTDIR=${quote(sessionDirectory)}\n`, { mode: 0o600 });
      await fs.writeFile(path.join(sessionDirectory, ".zshrc"), `ZDOTDIR=$__shucked_original_zdotdir\n[[ -r $ZDOTDIR/.zshrc ]] && source $ZDOTDIR/.zshrc\nsource ${quote(hook)}\n`, { mode: 0o600 });
      env.ZDOTDIR = sessionDirectory; shellArgs = ["-i"];
    } else { shellArgs = ["-i", "--init-command", `source ${fishQuote(hook)}`]; }
    const options: vscode.TerminalOptions = { name: `Shucked ${shell}`, shellPath: path.basename(process.env.SHELL ?? "") === shell ? process.env.SHELL : shell, shellArgs, env, cwd: vscode.workspace.workspaceFolders?.[0]?.uri };
    return { session, options };
  }

  private link(terminal: vscode.Terminal, id: string): void {
    const session = this.sessions.get(id); if (!session) { return; }
    session.terminal = terminal;
    void terminal.processId.then(pid => { if (pid) { session.pid ??= pid; } });
  }

  public async create(): Promise<void> {
    if (!vscode.workspace.isTrusted) { await vscode.window.showInformationMessage("Trust this workspace to create a Shucked terminal."); return; }
    const shell = await this.selectShell(); if (!shell) { return; }
    try { const { session, options } = await this.prepare(shell); const terminal = vscode.window.createTerminal(options); this.link(terminal, session.id); await this.environments.attachSession(session.id); terminal.show(); }
    catch { await vscode.window.showErrorMessage("Shucked could not initialize the terminal integration."); }
  }

  public async attach(): Promise<void> {
    if (!vscode.workspace.isTrusted) { return; }
    const terminal = vscode.window.activeTerminal; if (!terminal) { await this.create(); return; }
    const shell = await this.selectShell(); if (!shell) { return; }
    try {
      const { session, options } = await this.prepare(shell); this.link(terminal, session.id);
      const hook = path.join(this.context.extensionPath, "shell-integration", shell === "zsh" ? "zsh.zsh" : shell === "fish" ? "fish.fish" : "bash.sh");
      const attachFile = path.join(this.directory!, session.id, `attach.${shell}`);
      const variables = Object.entries(options.env ?? {}).filter(([key]) => key !== "ZDOTDIR").map(([key, value]) => shell === "fish" ? `set -gx ${key} ${fishQuote(value!)};` : `export ${key}=${quote(value!)};`).join("\n");
      await fs.writeFile(attachFile, `${variables}\nsource ${shell === "fish" ? fishQuote(hook) : quote(hook)}\n`, { mode: 0o600 });
      await vscode.env.clipboard.writeText(`source ${shell === "fish" ? fishQuote(attachFile) : quote(attachFile)}`);
      await this.environments.attachSession(session.id);
      await vscode.window.showInformationMessage("Shucked attachment command copied. Paste it at an idle shell prompt. No command was sent to the terminal.");
    } catch { await vscode.window.showErrorMessage("Shucked could not prepare terminal attachment."); }
  }

  private disconnect(session: AttachedSession): void {
    this.sessions.delete(session.id); this.history.clearSession(session.id);
    void this.client.notify("shucked/shellSession", { ...(session.metadata ?? { id: session.id, cwd: os.homedir(), path: [], aliases: {}, functions: [], options: {} }), generation: session.generation + 1, connected: false }).catch(() => undefined);
  }

  public dispose(): void {
    this.disposed = true;
    for (const subscription of this.subscriptions) { subscription.dispose(); }
    for (const session of this.sessions.values()) { this.disconnect(session); }
    this.server?.close();
    if (this.directory) { void fs.rm(this.directory, { recursive: true, force: true }).catch(() => undefined); }
  }
}
function quote(value: string): string { return `'${value.replace(/'/g, "'\\''")}'`; }
function fishQuote(value: string): string { return `'${value.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`; }
