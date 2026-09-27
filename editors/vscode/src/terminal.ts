import * as vscode from "vscode";
import * as net from "node:net";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { ClientManager } from "./client";
import { EnvironmentManager } from "./environment";
import { LIVE_SIGNALS, LiveCompletionManager, validLiveHelperHello, type LiveSignal } from "./live-completion";
import type { HistoryManager } from "./history";

type Shell = "bash" | "zsh" | "fish";
export interface SessionMetadata {
  id: string; generation: number; pid: number; shell: Shell; cwd: string; path: string[];
  aliases: Record<string, string[]>; functions: string[]; options: Record<string, string>;
  private: boolean; ignore: string[]; connected: boolean; acceptedHistoryHash?: string; historyFile?: string; liveCompletion?: boolean; liveSignal?: LiveSignal;
}
interface AttachedSession { id: string; token: string; shell: Shell; generation: number; terminal?: vscode.Terminal; metadata?: SessionMetadata; pid?: number; directory?: string; executing?: boolean; historyPolicy?: string; executionGeneration?: number; pendingHistory?: { text: string; generation: number }; dropWarned?: boolean; }
/** One hook payload; matches the cap in `shell-integration/capture.cjs`. */
export const MAX_FRAME = 1024 * 1024;
/** Aliases or functions accepted from one prompt; the server truncates at the same bound. */
export const MAX_NAMES = 50000;
/** Reasons a hook reports when it had to drop its payload instead of sending it. */
export const DROP_REASONS = ["size", "deadline"] as const;
export type DropReason = typeof DROP_REASONS[number];
const stringList = (value: unknown, max = MAX_NAMES): value is string[] => Array.isArray(value) && value.length <= max && value.every(item => typeof item === "string" && item.length < 16384 && !item.includes("\0"));

/** Validate untrusted local IPC before publishing any shell metadata. */
export function validateSessionMessage(value: unknown): value is SessionMetadata & { token: string } {
  if (!value || typeof value !== "object") { return false; }
  const item = value as Record<string, unknown>;
  if (typeof item.id !== "string" || !/^[a-f0-9]{32}$/.test(item.id) || typeof item.token !== "string" || !/^[a-f0-9]{64}$/.test(item.token)) { return false; }
  if (!Number.isSafeInteger(item.generation) || Number(item.generation) < 1 || !Number.isSafeInteger(item.pid) || Number(item.pid) <= 0) { return false; }
  if (!["bash", "zsh", "fish"].includes(String(item.shell)) || typeof item.cwd !== "string" || !path.isAbsolute(item.cwd) || item.cwd.includes("\0")) { return false; }
  if (!stringList(item.path, 1024) || !stringList(item.functions) || !stringList(item.ignore, 32)) { return false; }
  if (item.acceptedHistoryHash !== undefined && (typeof item.acceptedHistoryHash !== "string" || !/^[a-f0-9]{64}$/.test(item.acceptedHistoryHash))) { return false; }
  if (item.historyFile !== undefined && (typeof item.historyFile !== "string" || item.historyFile.length > 16384 || item.historyFile.includes("\0") || (item.historyFile !== "" && !path.isAbsolute(item.historyFile)))) { return false; }
  if (item.liveCompletion !== undefined && typeof item.liveCompletion !== "boolean") { return false; }
  if (item.liveSignal !== undefined && !LIVE_SIGNALS.includes(item.liveSignal as LiveSignal)) { return false; }
  if (typeof item.private !== "boolean" || item.connected !== true || !item.aliases || typeof item.aliases !== "object" || Array.isArray(item.aliases)) { return false; }
  if (Object.keys(item.aliases).length > MAX_NAMES || !Object.entries(item.aliases).every(([name, words]) => /^[\w.:-]{1,256}$/.test(name) && stringList(words, 32))) { return false; }
  return !!item.options && typeof item.options === "object" && !Array.isArray(item.options) && Object.entries(item.options).length < 128 && Object.entries(item.options).every(([key, value]) => /^[\w_-]+$/.test(key) && typeof value === "string" && value.length < 128);
}

/** A hook's notice that it dropped a prompt payload; authenticated like metadata. */
export interface DropNotice { kind: "hookDropped"; id: string; token: string; generation: number; pid: number; shell: Shell; reason: DropReason }

export function validateDropNotice(value: unknown): value is DropNotice {
  if (!value || typeof value !== "object") { return false; }
  const item = value as Record<string, unknown>;
  return item.kind === "hookDropped" && typeof item.id === "string" && /^[a-f0-9]{32}$/.test(item.id)
    && typeof item.token === "string" && /^[a-f0-9]{64}$/.test(item.token)
    && Number.isSafeInteger(item.generation) && Number(item.generation) >= 1 && Number.isSafeInteger(item.pid) && Number(item.pid) > 0
    && ["bash", "zsh", "fish"].includes(String(item.shell)) && DROP_REASONS.includes(item.reason as DropReason);
}

/** What the user is told the first time a terminal's state update is lost. */
export function dropWarning(shell: string, reason: string): string {
  const cause = reason === "size" ? `its prompt report exceeded ${MAX_FRAME / 1024} KiB`
    : reason === "deadline" ? "its prompt hook did not finish within the deadline"
      : `a state update failed ${reason} checks`;
  return `Shucked could not read the ${shell} terminal state because ${cause}. Aliases and functions from this terminal are unavailable until a later prompt succeeds.`;
}

export class TerminalManager implements vscode.Disposable {
  private readonly sessions = new Map<string, AttachedSession>();
  private readonly subscriptions: vscode.Disposable[] = [];
  private server?: net.Server;
  private directory?: string;
  private socketPath?: string;
  private starting?: Promise<void>;
  private disposed = false;
  private readonly live: LiveCompletionManager;

  constructor(private readonly context: vscode.ExtensionContext, private readonly client: ClientManager, private readonly environments: EnvironmentManager, private readonly history: HistoryManager, output?: vscode.LogOutputChannel) {
    this.live = new LiveCompletionManager(client, environments, id => this.sessions.get(id), output);
    this.subscriptions.push(
      this.live,
      client.onReady(() => { for (const session of this.sessions.values()) { if (session.metadata) { void this.client.notify("shucked/shellSession", session.metadata).catch(() => undefined); } } }),
      vscode.workspace.onDidChangeConfiguration(event => {
        if (event.affectsConfiguration("shucked.history")) { for (const session of this.sessions.values()) { session.pendingHistory = undefined; if (session.historyPolicy) { void fs.writeFile(session.historyPolicy, `${this.history.sessionEnabled() ? 1 : 0}\n${this.history.filesEnabled() ? 1 : 0}\n`, { mode: 0o600 }).catch(() => undefined); } } }
      }),
      vscode.window.onDidStartTerminalShellExecution(event => {
        const session = [...this.sessions.values()].find(session => session.terminal === event.terminal);
        if (session) { session.executing = true; this.live.cancelSession(session.id); if (this.history.sessionEnabled()) { session.executionGeneration = session.generation; } }
      }),
      vscode.commands.registerCommand("shucked.createTerminal", (shell?: Shell) => this.create(shell)),
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
        const pending = { text: event.execution.commandLine.value, generation: session.executionGeneration ?? session.generation };
        if (session.generation > pending.generation) { this.history.recordSession(session.id, pending.text, session.metadata); }
        else { session.pendingHistory = pending; }
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
          if (handled) { socket.destroy(); return; }
          if (frame.length + chunk.length > MAX_FRAME) { socket.destroy(); this.reportDrop(undefined, "size"); return; }
          frame = Buffer.concat([frame, chunk]);
          if (!frame.includes(10)) { return; }
          handled = true;
          try {
            const value: unknown = JSON.parse(frame.subarray(0, frame.indexOf(10)).toString("utf8"));
            // A terminal's live completion helper keeps its connection open; every other sender delivers one frame.
            if (validLiveHelperHello(value) && this.live.adopt(value, socket, frame.subarray(frame.indexOf(10) + 1))) { return; }
            if (validateDropNotice(value)) { this.dropped(value); }
            else if (validateSessionMessage(value)) { void this.receive(value).catch(() => undefined); }
            else if (value && typeof value === "object" && "id" in value && typeof value.id === "string") { this.reportDrop(this.sessions.get(value.id), "validation"); }
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

  /** Authenticate a session message; a mismatch is reported once, never trusted. */
  private authenticate(session: AttachedSession | undefined, shell: Shell, token: string, pid: number): session is AttachedSession {
    if (this.disposed || !vscode.workspace.isTrusted || !session) { return false; }
    if (session.shell !== shell) { this.reportDrop(session, "shell identity"); return false; }
    const expected = Buffer.from(session.token), actual = Buffer.from(token);
    if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) { this.reportDrop(session, "authentication"); return false; }
    if (session.pid && session.pid !== pid) { this.reportDrop(session, "process identity"); return false; }
    return true;
  }

  /** A hook could not deliver its payload (too large or too slow); say so once per terminal. */
  private dropped(notice: DropNotice): void {
    const session = this.sessions.get(notice.id);
    if (!this.authenticate(session, notice.shell, notice.token, notice.pid) || notice.generation <= session.generation) { return; }
    this.reportDrop(session, notice.reason);
  }

  private generalDropWarned = false;
  private reportDrop(session: AttachedSession | undefined, reason: string): void {
    if (session) {
      if (session.dropWarned) { return; }
      session.dropWarned = true;
      this.environments.sessionState(session.id, session.metadata ? true : undefined, session.metadata, reason);
      void vscode.window.showWarningMessage(dropWarning(session.shell, reason));
    } else {
      if (this.generalDropWarned) { return; }
      this.generalDropWarned = true;
      void vscode.window.showWarningMessage(dropWarning("attached", reason));
    }
  }

  private async receive(message: SessionMetadata & { token: string }): Promise<void> {
    const session = this.sessions.get(message.id);
    const { token, ...metadata } = message;
    if (!this.authenticate(session, message.shell, token, message.pid) || message.generation <= session.generation) { return; }
    session.pid ??= message.pid;
    this.live.cancelSession(session.id);
    session.executing = false;
    session.generation = message.generation;
    if (process.platform === "win32") { metadata.liveCompletion = false; }
    session.metadata = metadata;
    this.history.updateSession(metadata);
    this.environments.sessionState(session.id, true, metadata);
    if (session.pendingHistory && metadata.generation > session.pendingHistory.generation) {
      this.history.recordSession(session.id, session.pendingHistory.text, metadata); session.pendingHistory = undefined;
    }
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
    this.environments.sessionState(session.id, undefined);
    const integration = path.join(this.context.extensionPath, "shell-integration");
    const hook = path.join(integration, shell === "zsh" ? "zsh.zsh" : shell === "fish" ? "fish.fish" : "bash.sh");
    const env: Record<string, string> = { SHUCKED_SESSION_ID: session.id, SHUCKED_SESSION_TOKEN: session.token, SHUCKED_SESSION_SOCKET: this.socketPath!, SHUCKED_NODE: process.execPath, SHUCKED_CAPTURE: path.join(integration, "capture.cjs") };
    const sessionDirectory = path.join(this.directory!, session.id); await fs.mkdir(sessionDirectory, { mode: 0o700 });
    session.directory = sessionDirectory;
    Object.assign(env, { SHUCKED_LIVE_ALLOWED: process.platform === "win32" ? "0" : "1", SHUCKED_LIVE_DIRECTORY: sessionDirectory, SHUCKED_LIVE_HELPER: path.join(integration, "live-helper.cjs") });
    session.historyPolicy = path.join(sessionDirectory, "history-policy");
    await fs.writeFile(session.historyPolicy, `${this.history.sessionEnabled() ? 1 : 0}\n${this.history.filesEnabled() ? 1 : 0}\n`, { mode: 0o600 });
    env.SHUCKED_HISTORY_POLICY = session.historyPolicy;
    let shellArgs: string[];
    if (shell === "bash") {
      const init = path.join(sessionDirectory, "bashrc");
      await fs.writeFile(init, bashLoginInit(hook), { mode: 0o600 });
      shellArgs = ["--rcfile", init, "-i"];
    } else if (shell === "zsh") {
      const original = process.env.ZDOTDIR ?? os.homedir();
      for (const [name, contents] of Object.entries(zshLoginFiles(original, sessionDirectory, hook))) {
        await fs.writeFile(path.join(sessionDirectory, name), contents, { mode: 0o600 });
      }
      env.ZDOTDIR = sessionDirectory; shellArgs = ["-l", "-i"];
    } else { shellArgs = ["-l", "-i", "--init-command", `source ${fishQuote(hook)}`]; }
    const options: vscode.TerminalOptions = { name: `Shucked ${shell}`, shellPath: path.basename(process.env.SHELL ?? "") === shell ? process.env.SHELL : shell, shellArgs, env, cwd: vscode.workspace.workspaceFolders?.[0]?.uri };
    return { session, options };
  }

  private link(terminal: vscode.Terminal, id: string): void {
    const session = this.sessions.get(id); if (!session) { return; }
    session.terminal = terminal;
    void terminal.processId.then(pid => { if (pid) { session.pid ??= pid; } });
  }

  public async create(selectedShell?: Shell): Promise<void> {
    if (!vscode.workspace.isTrusted) { await vscode.window.showInformationMessage("Trust this workspace to create a Shucked terminal."); return; }
    const shell = selectedShell && ["bash", "zsh", "fish"].includes(selectedShell) ? selectedShell : await this.selectShell(); if (!shell) { return; }
    try { const { session, options } = await this.prepare(shell); const terminal = vscode.window.createTerminal(options); this.link(terminal, session.id); await this.environments.attachSession(session.id); terminal.show(); }
    catch { await vscode.window.showErrorMessage("Shucked could not initialize the terminal integration."); }
  }

  public async attach(): Promise<void> {
    if (!vscode.workspace.isTrusted) { return; }
    const terminal = vscode.window.activeTerminal; if (!terminal) { await this.create(); return; }
    const shell = await this.selectShell(); if (!shell) { return; }
    try {
      const { session, options } = await this.prepare(shell); session.terminal = terminal;
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
    this.live.cancelSession(session.id);
    this.live.stopHelper(session.id);
    this.sessions.delete(session.id); this.history.clearSession(session.id);
    this.environments.sessionState(session.id, false, session.metadata);
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
/**
 * Private rc file for a Shucked bash terminal. Bash ignores `--rcfile` for
 * login shells, so the file performs the login startup order itself:
 * /etc/profile, then the first of ~/.bash_profile, ~/.bash_login and
 * ~/.profile (falling back to ~/.bashrc when none exists), then the hook.
 */
export function bashLoginInit(hook: string): string {
  return [
    "[[ -f /etc/profile ]] && source /etc/profile",
    "__shucked_profile_read=0",
    "for __shucked_profile in ~/.bash_profile ~/.bash_login ~/.profile; do",
    "  if [[ -f $__shucked_profile ]]; then source \"$__shucked_profile\"; __shucked_profile_read=1; break; fi",
    "done",
    "[[ $__shucked_profile_read = 0 && -f ~/.bashrc ]] && source ~/.bashrc",
    "unset __shucked_profile __shucked_profile_read",
    `source ${quote(hook)}`,
    "",
  ].join("\n");
}

/**
 * Private ZDOTDIR files for a Shucked zsh login shell. Each one hands control
 * to the user's file of the same name and then points ZDOTDIR back at the
 * private directory, so .zshenv, .zprofile, .zshrc and .zlogin all run in
 * login order and the hook is sourced after .zshrc.
 */
export function zshLoginFiles(original: string, sessionDirectory: string, hook: string): Record<string, string> {
  return {
    ".zshenv": `ZDOTDIR=${quote(original)}\n[[ -r $ZDOTDIR/.zshenv ]] && source $ZDOTDIR/.zshenv\n__shucked_original_zdotdir=$ZDOTDIR\nZDOTDIR=${quote(sessionDirectory)}\n`,
    ".zprofile": `ZDOTDIR=$__shucked_original_zdotdir\n[[ -r $ZDOTDIR/.zprofile ]] && source $ZDOTDIR/.zprofile\nZDOTDIR=${quote(sessionDirectory)}\n`,
    ".zshrc": `ZDOTDIR=$__shucked_original_zdotdir\n[[ -r $ZDOTDIR/.zshrc ]] && source $ZDOTDIR/.zshrc\nsource ${quote(hook)}\n`,
  };
}
function quote(value: string): string { return `'${value.replace(/'/g, "'\\''")}'`; }
function fishQuote(value: string): string { return `'${value.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`; }
