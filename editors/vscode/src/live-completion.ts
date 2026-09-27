import * as vscode from "vscode";
import type * as net from "node:net";
import { randomBytes, timingSafeEqual } from "node:crypto";
import type { ClientManager } from "./client";
import type { EnvironmentManager } from "./environment";
import type { SessionMetadata } from "./terminal";

export interface LiveSession {
  id: string; token: string; generation: number; shell: string;
  pid?: number; directory?: string; metadata?: SessionMetadata; executing?: boolean;
}
export interface LiveCompletionParams {
  uri: string; version: number; sessionId: string; generation: number;
  dialect: string; words: string[]; prefix: string;
}
export interface LiveCompletionResponse { candidates: { text: string; description: string; encoding?: "bashWord" }[]; partial: boolean; reason?: string; }
/** The first frame a terminal's persistent helper (`shell-integration/live-helper.cjs`) sends. */
export interface LiveHelperHello { kind: "liveHelper"; phase: "hello"; id: string; token: string; shell: string; pid: number; shellPid: number; signal: string; }
/** Signals a hook may reserve for live requests; the helper sends the one the shell chose. */
export const LIVE_SIGNALS = ["SIGUSR1", "SIGUSR2", "SIGWINCH"] as const;
export type LiveSignal = typeof LIVE_SIGNALS[number];
/** One helper frame: a result carries at most 2000 bounded candidates. */
const MAX_HELPER_FRAME = 1024 * 1024;
/** The server gives up at 1500 ms; the helper stops a worker at 1250 ms. */
const DEADLINE = 1300;
interface Helper { socket: net.Socket; pid: number; shellPid: number; shell: string; signal: string; buffer: Buffer; }
interface Pending {
  query: string; params: LiveCompletionParams; session: LiveSession; selection: string; started: number;
  cancelled: boolean; finish: (reply: LiveCompletionResponse, answered?: boolean, elapsedMs?: number) => void;
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
export function validLiveHelperHello(value: unknown): value is LiveHelperHello {
  if (!value || typeof value !== "object") { return false; }
  const item = value as Record<string, unknown>;
  return item.kind === "liveHelper" && item.phase === "hello"
    && typeof item.id === "string" && /^[a-f0-9]{32}$/.test(item.id) && typeof item.token === "string" && /^[a-f0-9]{64}$/.test(item.token)
    && ["bash", "zsh", "fish"].includes(String(item.shell))
    && Number.isSafeInteger(item.pid) && Number(item.pid) > 1 && Number.isSafeInteger(item.shellPid) && Number(item.shellPid) > 1
    && LIVE_SIGNALS.includes(item.signal as LiveSignal);
}

/**
 * Executes only callbacks in explicitly attached shell state, with bounded IPC.
 *
 * Each attached terminal runs one persistent helper process that keeps a
 * connection here. A request is one JSON frame down that connection; the
 * helper writes it into the session's private directory, signals the shell,
 * and streams the shell worker's result back as one frame. Cancellation and
 * the deadline are frames too, so no process table is inspected here.
 */
export class LiveCompletionManager implements vscode.Disposable {
  private readonly pending = new Map<string, Pending>();
  private readonly helpers = new Map<string, Helper>();
  private readonly registration: vscode.Disposable;
  private disposed = false;
  constructor(client: ClientManager, private readonly environments: EnvironmentManager, private readonly session: (id: string) => LiveSession | undefined, private readonly output?: vscode.LogOutputChannel) {
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
  private write(helper: Helper, frame: Record<string, unknown>): boolean {
    if (helper.socket.destroyed) { return false; }
    try { helper.socket.write(JSON.stringify(frame) + "\n"); return true; } catch { return false; }
  }
  private async request(value: unknown, cancellation: vscode.CancellationToken): Promise<LiveCompletionResponse> {
    if (!validLiveParams(value) || process.platform === "win32" || !vscode.workspace.isTrusted || this.disposed) { return unavailable("Live shell completion is unavailable"); }
    const params = value;
    const session = this.session(params.sessionId);
    const document = vscode.workspace.textDocuments.find(document => document.uri.toString() === params.uri);
    const selection = document && this.environments.selection(document);
    if (!session?.metadata?.liveCompletion || !session.pid || !session.directory
      || session.executing || session.shell !== params.dialect || session.generation !== params.generation
      || !document || document.version !== params.version || selection?.sessionId !== params.sessionId
      || selection.policy === "portable" || selection.targetInventory
      || /(?:^|[/\\])(?:\.zshrc|\.zshenv|\.zprofile|\.bashrc|\.bash_profile|\.profile|config\.fish)$/.test(document.fileName)) { return unavailable("The selected shell is not available for live completion"); }
    const helper = this.helpers.get(params.sessionId);
    if (!helper || helper.socket.destroyed || helper.shellPid !== session.pid || helper.shell !== session.shell) { return unavailable("The live completion helper is not connected"); }
    for (const previous of this.pending.values()) { if (previous.params.sessionId === params.sessionId && !previous.cancelled) { previous.finish(unavailable("Superseded completion")); } }
    if (this.pending.size >= 128) { return unavailable("Live completion request limit reached"); }
    const query = randomBytes(16).toString("hex");
    return new Promise(resolve => {
      let finished = false;
      let cancellationSubscription: vscode.Disposable = { dispose: () => undefined };
      const pending: Pending = { query, params, session: { ...session }, selection: JSON.stringify(selection), started: performance.now(), cancelled: false, finish: (reply, answered = false, elapsedMs) => {
        if (finished) { return; } finished = true;
        const valid = this.current(pending);
        pending.cancelled = true; clearTimeout(timer); cancellationSubscription.dispose();
        this.pending.delete(query);
        if (!answered) { this.write(helper, { kind: "cancel", query }); }
        const total = Math.round(performance.now() - pending.started);
        this.output?.debug(answered
          ? `Live completion (${session.shell}): ${total}ms round trip, ${elapsedMs ?? "?"}ms in the shell, ${reply.candidates.length} candidates${reply.partial ? ` (partial: ${reply.reason ?? "no reason"})` : ""}`
          : `Live completion (${session.shell}): no result after ${total}ms (${reply.reason ?? "cancelled"})`);
        resolve(valid ? reply : unavailable("Completion context changed"));
      } };
      this.pending.set(query, pending);
      const timer = setTimeout(() => pending.finish(unavailable("Live completion timed out")), DEADLINE);
      cancellationSubscription = cancellation.onCancellationRequested(() => pending.finish(unavailable("Completion cancelled")));
      if (cancellation.isCancellationRequested) { pending.finish(unavailable("Completion cancelled")); return; }
      if (!this.write(helper, { kind: "request", query, generation: params.generation, prefix: params.prefix, words: params.words })) { pending.finish(unavailable("The live completion helper is not connected")); }
    });
  }
  /**
   * Take over a connection whose first frame introduced a terminal's helper.
   * Returns false when the greeting is not for a known session or fails
   * authentication; the caller then treats the connection as any other.
   */
  public adopt(value: unknown, socket: net.Socket, rest: Buffer): boolean {
    if (!validLiveHelperHello(value) || process.platform === "win32" || this.disposed || !vscode.workspace.isTrusted) { return false; }
    const session = this.session(value.id);
    if (!session || session.shell !== value.shell) { return false; }
    const expected = Buffer.from(session.token), actual = Buffer.from(value.token);
    if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) { return false; }
    if (session.pid !== undefined && session.pid !== value.shellPid) { return false; }
    this.stopHelper(value.id);
    const helper: Helper = { socket, pid: value.pid, shellPid: value.shellPid, shell: value.shell, signal: value.signal, buffer: Buffer.alloc(0) };
    this.helpers.set(value.id, helper);
    socket.setTimeout(0);
    socket.removeAllListeners("data");
    socket.on("data", chunk => this.frames(helper, chunk));
    socket.on("close", () => {
      if (this.helpers.get(value.id) !== helper) { return; }
      this.helpers.delete(value.id);
      this.output?.debug(`Live completion helper (${value.shell}, pid ${value.pid}) disconnected`);
      for (const pending of this.pending.values()) { if (pending.params.sessionId === value.id) { pending.finish(unavailable("The live completion helper disconnected")); } }
    });
    this.output?.debug(`Live completion helper (${value.shell}, pid ${value.pid}) connected for shell ${value.shellPid} using ${value.signal}`);
    if (rest.length) { this.frames(helper, rest); }
    return true;
  }
  private frames(helper: Helper, chunk: Buffer): void {
    helper.buffer = helper.buffer.length ? Buffer.concat([helper.buffer, chunk]) : chunk;
    if (helper.buffer.length > MAX_HELPER_FRAME) { helper.socket.destroy(); return; }
    let at: number;
    while ((at = helper.buffer.indexOf(10)) >= 0) {
      const line = helper.buffer.subarray(0, at);
      helper.buffer = helper.buffer.subarray(at + 1);
      try { this.receive(JSON.parse(line.toString("utf8"))); } catch { /* Malformed data is discarded without logging shell contents. */ }
    }
  }
  /** A helper's result frame; authenticated against the session that asked. */
  public receive(value: unknown): void {
    if (!value || typeof value !== "object") { return; }
    const message = value as Record<string, unknown>;
    if (message.kind !== "liveCompletion" || typeof message.query !== "string" || typeof message.token !== "string") { return; }
    const pending = this.pending.get(message.query);
    if (!pending || message.id !== pending.session.id || message.generation !== pending.params.generation) { return; }
    const expected = Buffer.from(pending.session.token), actual = Buffer.from(message.token);
    if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) { return; }
    if (message.phase !== "result" || !Array.isArray(message.candidates) || message.candidates.length > 2000 || typeof message.partial !== "boolean") { return; }
    let size = 0;
    const valid = message.candidates.every(item => {
      if (!item || (item.encoding !== undefined && item.encoding !== "bashWord") || typeof item.text !== "string" || typeof item.description !== "string" || item.text.includes("\0") || item.text.length > 8192 || item.description.length > 16384) { return false; }
      size += Buffer.byteLength(item.text) + Buffer.byteLength(item.description); return size <= 192 * 1024;
    });
    if (valid) {
      const elapsed = Number.isSafeInteger(message.elapsedMs) && Number(message.elapsedMs) >= 0 ? Number(message.elapsedMs) : undefined;
      pending.finish({ candidates: message.candidates, partial: message.partial, reason: typeof message.reason === "string" ? message.reason.slice(0, 512) : undefined }, true, elapsed);
    }
  }
  /** Whether a terminal's helper is connected (for diagnostics and tests). */
  public helperConnected(id: string): boolean { const helper = this.helpers.get(id); return !!helper && !helper.socket.destroyed; }
  public cancelSession(id: string): void { for (const pending of this.pending.values()) { if (pending.params.sessionId === id) { pending.finish(unavailable("Shell session changed")); } } }
  /** Ask a terminal's helper to exit; used when the session is disconnected. */
  public stopHelper(id: string): void {
    const helper = this.helpers.get(id);
    if (!helper) { return; }
    this.helpers.delete(id);
    this.write(helper, { kind: "stop" });
    helper.socket.end();
  }
  public dispose(): void {
    this.disposed = true; this.registration.dispose();
    for (const pending of this.pending.values()) { pending.finish(unavailable("Live completion stopped")); }
    for (const id of [...this.helpers.keys()]) { this.stopHelper(id); }
  }
}
