import * as vscode from "vscode";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { randomBytes, timingSafeEqual } from "node:crypto";
import type { ClientManager } from "./client";
import type { EnvironmentManager } from "./environment";
import type { SessionMetadata } from "./terminal";

const execute = promisify(execFile);
export interface LiveSession {
  id: string; token: string; generation: number; shell: string;
  pid?: number; directory?: string; metadata?: SessionMetadata; executing?: boolean;
}
export interface LiveCompletionParams {
  uri: string; version: number; sessionId: string; generation: number;
  dialect: string; words: string[]; prefix: string;
}
export interface LiveCompletionResponse { candidates: { text: string; description: string; encoding?: "bashWord" }[]; partial: boolean; reason?: string; }
interface Identity { pid: number; parent: number; started: string; }
interface Pending {
  query: string; params: LiveCompletionParams; session: LiveSession; filename: string;
  selection: string; identity?: Identity; cancelled: boolean; finish: (reply: LiveCompletionResponse) => void;
}
const unavailable = (reason: string): LiveCompletionResponse => ({ candidates: [], partial: true, reason });

export function validLiveParams(value: unknown): value is LiveCompletionParams {
  if (!value || typeof value !== "object") { return false; }
  const item = value as LiveCompletionParams;
  return typeof item.uri === "string" && Number.isSafeInteger(item.version)
    && typeof item.sessionId === "string" && /^[a-f0-9]{32}$/.test(item.sessionId)
    && Number.isSafeInteger(item.generation) && item.generation > 0
    && ["bash", "zsh", "fish"].includes(item.dialect)
    && typeof item.prefix === "string" && !item.prefix.includes("\0") && Buffer.byteLength(item.prefix) <= 8192
    && Array.isArray(item.words) && item.words.length > 0 && item.words.length <= 256
    && item.words.every(word => typeof word === "string" && !word.includes("\0") && Buffer.byteLength(word) <= 8192)
    && Buffer.byteLength([...item.words, item.prefix].join("\0")) <= 30000;
}
async function processTable(): Promise<Identity[]> {
  const result = await execute("/bin/ps", ["-axo", "pid=,ppid=,lstart="], { timeout: 150, maxBuffer: 1024 * 1024 });
  return result.stdout.split("\n").flatMap(line => {
    const match = line.trim().match(/^(\d+)\s+(\d+)\s+(.+)$/);
    return match ? [{ pid: Number(match[1]), parent: Number(match[2]), started: match[3]! }] : [];
  });
}
async function stopWorker(identity: Identity): Promise<void> {
  try {
    const table = await processTable();
    if (!table.some(item => item.pid === identity.pid && item.started === identity.started && item.parent === identity.parent)) { return; }
    const owned = new Set([identity.pid]);
    for (let depth = 0; depth < 32; depth++) {
      const before = owned.size; for (const item of table) { if (owned.has(item.parent)) { owned.add(item.pid); } }
      if (before === owned.size) { break; }
    }
    for (const pid of [...owned].reverse()) { try { process.kill(pid, "SIGKILL"); } catch { /* Worker exited. */ } }
  } catch { /* Never signal an unverified process. */ }
}

/** Executes only callbacks in explicitly attached shell state, with bounded IPC. */
export class LiveCompletionManager implements vscode.Disposable {
  private readonly pending = new Map<string, Pending>();
  private readonly registration: vscode.Disposable;
  private disposed = false;
  constructor(client: ClientManager, private readonly environments: EnvironmentManager, private readonly session: (id: string) => LiveSession | undefined) {
    this.registration = client.onRequest("shucked/liveCompletion", (params, cancellation) => this.request(params, cancellation));
  }
  private current(pending: Pending): boolean {
    const document = vscode.workspace.textDocuments.find(document => document.uri.toString() === pending.params.uri);
    const session = this.session(pending.params.sessionId);
    return !this.disposed && !pending.cancelled && vscode.workspace.isTrusted
      && !!document && document.version === pending.params.version
      && JSON.stringify(this.environments.selection(document)) === pending.selection
      && session?.metadata?.liveCompletion === true && !session.executing
      && session.generation === pending.params.generation && session.pid === pending.session.pid;
  }
  private async request(value: unknown, cancellation: vscode.CancellationToken): Promise<LiveCompletionResponse> {
    if (!validLiveParams(value) || process.platform === "win32" || !vscode.workspace.isTrusted || this.disposed) { return unavailable("Live shell completion is unavailable"); }
    const params = value;
    const session = this.session(params.sessionId);
    const document = vscode.workspace.textDocuments.find(document => document.uri.toString() === params.uri);
    const selection = document && this.environments.selection(document);
    if (!session?.metadata?.liveCompletion || !session.metadata.liveSignal || !session.pid || !session.directory
      || session.executing || session.shell !== params.dialect || session.generation !== params.generation
      || !document || document.version !== params.version || selection?.sessionId !== params.sessionId
      || selection.policy === "portable" || selection.targetInventory
      || /(?:^|[/\\])(?:\.zshrc|\.zshenv|\.zprofile|\.bashrc|\.bash_profile|\.profile|config\.fish)$/.test(document.fileName)) { return unavailable("The selected shell is not available for live completion"); }
    for (const previous of this.pending.values()) { if (previous.params.sessionId === params.sessionId && !previous.cancelled) { previous.finish(unavailable("Superseded completion")); } }
    if (this.pending.size >= 128) { return unavailable("Live completion request limit reached"); }
    const query = randomBytes(16).toString("hex");
    const filename = path.join(session.directory, `request-${query}`);
    return new Promise(resolve => {
      let finished = false;
      let cancellationSubscription: vscode.Disposable = { dispose: () => undefined };
      const pending: Pending = { query, params, session: { ...session }, filename, selection: JSON.stringify(selection), cancelled: false, finish: reply => {
        if (finished) { return; } finished = true;
        const valid = this.current(pending);
        pending.cancelled = true; clearTimeout(timer); cancellationSubscription.dispose();
        void fs.rm(filename, { force: true });
        if (pending.identity) { void stopWorker(pending.identity); }
        // Retain authentication briefly to kill a worker started during cancellation.
        setTimeout(() => this.pending.delete(query), 2500).unref();
        resolve(valid ? reply : unavailable("Completion context changed"));
      } };
      this.pending.set(query, pending);
      const timer = setTimeout(() => pending.finish(unavailable("Live completion timed out")), 1300);
      cancellationSubscription = cancellation.onCancellationRequested(() => pending.finish(unavailable("Completion cancelled")));
      if (cancellation.isCancellationRequested) { pending.finish(unavailable("Completion cancelled")); return; }
      const body = [query, String(params.generation), params.prefix, String(params.words.length), ...params.words].join("\0") + "\0";
      void fs.writeFile(filename, body, { flag: "wx", mode: 0o600 }).then(() => {
        if (!this.current(pending)) {
          // Cancellation may have removed the path before writeFile created it.
          void fs.rm(filename, { force: true });
          pending.finish(unavailable("Completion context changed")); return;
        }
        try { process.kill(session.pid!, session.metadata!.liveSignal); } catch { pending.finish(unavailable("The shell query hook is unavailable")); }
      }).catch(() => pending.finish(unavailable("Could not prepare live completion")));
    });
  }
  public async receive(value: unknown): Promise<void> {
    if (!value || typeof value !== "object") { return; }
    const message = value as Record<string, unknown>;
    if (message.kind !== "liveCompletion" || typeof message.query !== "string" || typeof message.token !== "string") { return; }
    const pending = this.pending.get(message.query);
    if (!pending || message.id !== pending.session.id || message.generation !== pending.params.generation) { return; }
    const expected = Buffer.from(pending.session.token), actual = Buffer.from(message.token);
    if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) { return; }
    if (message.phase === "started" && Number.isSafeInteger(message.pid) && Number(message.pid) > 0) {
      try {
        const identity = (await processTable()).find(item => item.pid === message.pid && item.parent === pending.session.pid);
        if (identity) { pending.identity = identity; if (pending.cancelled) { await stopWorker(identity); } }
      } catch { /* A worker that exited before inspection has nothing to cancel. */ }
      return;
    }
    if (message.phase !== "result" || !Array.isArray(message.candidates) || message.candidates.length > 2000 || typeof message.partial !== "boolean") { return; }
    let size = 0;
    const valid = message.candidates.every(item => {
      if (!item || (item.encoding !== undefined && item.encoding !== "bashWord") || typeof item.text !== "string" || typeof item.description !== "string" || item.text.includes("\0") || item.text.length > 8192 || item.description.length > 16384) { return false; }
      size += Buffer.byteLength(item.text) + Buffer.byteLength(item.description); return size <= 192 * 1024;
    });
    if (valid) { pending.finish({ candidates: message.candidates, partial: message.partial, reason: typeof message.reason === "string" ? message.reason.slice(0, 512) : undefined }); }
  }
  public cancelSession(id: string): void { for (const pending of this.pending.values()) { if (pending.params.sessionId === id) { pending.finish(unavailable("Shell session changed")); } } }
  public dispose(): void { this.disposed = true; this.registration.dispose(); for (const pending of this.pending.values()) { pending.finish(unavailable("Live completion stopped")); } }
}
