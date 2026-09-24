import * as vscode from "vscode";
import type { ProvideCompletionItemsSignature } from "vscode-languageclient/node";

interface CompletionReady {
  uri: string;
  version: number;
  position: { line: number; character: number };
  generation: number;
  provider?: string;
  elapsedMs?: number;
  candidateCount?: number;
  reason?: string;
}

interface PendingCompletion {
  uri: string;
  version: number;
  position: vscode.Position;
  generations: Set<number>;
  enrichments: number;
  queries: number;
  invalidations: number;
  responses: number;
  ready?: CompletionReady;
  settled: boolean;
  refreshing: boolean;
  timer?: ReturnType<typeof setTimeout>;
  document: vscode.TextDocument;
  next: ProvideCompletionItemsSignature;
  displayed: string;
  prepared?: vscode.CompletionItem[] | vscode.CompletionList;
  cancellation?: vscode.CancellationTokenSource;
}

type CompletionResult = vscode.CompletionItem[] | vscode.CompletionList | null | undefined;

function items(result: CompletionResult): vscode.CompletionItem[] {
  return Array.isArray(result) ? result : result?.items ?? [];
}

function fingerprint(result: CompletionResult): string {
  return JSON.stringify(items(result).map(item => [item.label, item.kind, item.detail,
    item.insertText, item.filterText, item.sortText, item.range, item.additionalTextEdits]));
}

function validReady(value: unknown): value is CompletionReady {
  if (!value || typeof value !== "object") { return false; }
  const ready = value as Partial<CompletionReady>;
  return typeof ready.uri === "string" && Number.isInteger(ready.version)
    && Number.isInteger(ready.generation) && !!ready.position
    && Number.isInteger(ready.position.line) && ready.position.line >= 0
    && Number.isInteger(ready.position.character) && ready.position.character >= 0;
}

/** Refresh only the typing session that requested delayed native candidates. */
export class CompletionRefresh implements vscode.Disposable {
  private pending: PendingCompletion | undefined;
  private readonly subscriptions: vscode.Disposable[];

  constructor(private readonly output: vscode.LogOutputChannel) {
    this.subscriptions = [
      vscode.commands.registerCommand("shucked.dismissCompletion", async () => {
        this.clear();
        await vscode.commands.executeCommand("hideSuggestWidget");
      }),
      vscode.commands.registerCommand("shucked.navigateCompletion", async (command: string) => {
        if (!["selectNextSuggestion", "selectPrevSuggestion", "selectNextPageSuggestion", "selectPrevPageSuggestion"].includes(command)) { return; }
        // Preserve deliberate keyboard selection while a provider is still busy.
        this.clear();
        await vscode.commands.executeCommand(command);
      }),
      vscode.workspace.onDidChangeTextDocument(event => {
        if (event.document.uri.toString() === this.pending?.uri) { this.clear(); }
      }),
      vscode.window.onDidChangeTextEditorSelection(() => {
        if (this.pending && !this.current(this.pending)) { this.clear(); }
      }),
      vscode.window.onDidChangeActiveTextEditor(() => this.clear()),
      vscode.window.onDidChangeWindowState(state => { if (!state.focused) { this.clear(); } }),
    ];
  }

  public async provide(
    document: vscode.TextDocument, position: vscode.Position, context: vscode.CompletionContext,
    token: vscode.CancellationToken, next: ProvideCompletionItemsSignature,
  ): Promise<vscode.CompletionItem[] | vscode.CompletionList | null | undefined> {
    const candidate: PendingCompletion = {
      uri: document.uri.toString(), version: document.version, position,
      generations: new Set(), enrichments: 0, queries: 0, invalidations: 0, responses: 0, settled: false, refreshing: false,
      document, next, displayed: "[]",
    };
    // Programmatic provider queries must not open a suggestion popup elsewhere.
    if (!this.current(candidate)) { return next(document, position, context, token); }
    let pending = this.pending;
    if (!pending || !this.matches(pending, candidate)) {
      this.clear();
      pending = candidate;
      this.pending = pending;
      // Bound retention when a provider fails without a ready notification.
      pending.timer = setTimeout(() => { if (this.pending === pending) { this.clear(); } }, 5000);
      void vscode.commands.executeCommand("setContext", "shucked.completionPending", true);
    }
    pending.next = next;
    pending.settled = false;
    const started = performance.now();
    const cancellation = token.onCancellationRequested(() => {
      if (this.pending === pending && !pending.settled && !pending.refreshing) { this.clear(); }
    });
    try {
      const prepared = pending.prepared;
      pending.prepared = undefined;
      const result = prepared ?? await next(document, position, context, token);
      this.output.trace(`Completion response: ${Math.round(performance.now() - started)}ms, ${Array.isArray(result) ? result.length : result?.items.length ?? 0} candidates`);
      if (this.pending === pending) {
        pending.settled = true;
        pending.displayed = fingerprint(result);
        if (pending.responses++ === 0 && result && !Array.isArray(result) && result.isIncomplete && result.items.length === 0) {
          // A cold workspace can need longer to prepare its semantic index.
          // Extend once; retries must never keep an abandoned session alive.
          if (pending.timer) { clearTimeout(pending.timer); }
          pending.timer = setTimeout(() => { if (this.pending === pending) { this.clear(); } }, 30000);
        }
        if (!result || Array.isArray(result) || !result.isIncomplete) { this.clear(); }
        else if (pending.ready) {
          // Let the initial provider response settle before asking VS Code again.
          setTimeout(() => { if (this.pending === pending && pending.ready) { this.ready(pending.ready); } }, 0);
        }
      }
      return result;
    } catch (error) {
      if (this.pending === pending) { this.clear(); }
      throw error;
    } finally { cancellation.dispose(); }
  }

  public ready(value: unknown): void {
    if (!validReady(value)) { return; }
    const pending = this.pending;
    const invalidated = value.reason === "environmentChanged";
    if (!pending || !this.matches(pending, value) || !this.current(pending)
      || pending.generations.has(value.generation)
      || pending.queries >= 12
      || (invalidated ? pending.invalidations >= 8 : pending.enrichments >= 3)) { return; }
    if (!pending.settled || pending.refreshing) { pending.ready = value; return; }
    pending.ready = undefined;
    pending.generations.add(value.generation);
    if (invalidated) { pending.invalidations++; }
    pending.queries++;
    pending.refreshing = true;
    this.output.trace(`Completion ready: ${value.provider ?? value.reason ?? "provider"}, ${value.elapsedMs ?? "?"}ms, ${value.candidateCount ?? "?"} candidates`);
    // Analysis/environment notices can arrive before any useful items exist.
    // Query privately first so those notices do not redraw an empty popup.
    const cancellation = new vscode.CancellationTokenSource();
    pending.cancellation = cancellation;
    void Promise.resolve().then(() => this.pending === pending && this.current(pending)
      ? pending.next(pending.document, pending.position,
        { triggerKind: vscode.CompletionTriggerKind.Invoke, triggerCharacter: undefined }, cancellation.token)
      : undefined).then(async result => {
      if (this.pending !== pending || !this.current(pending)) { return; }
      if (items(result).length > 0 && fingerprint(result) !== pending.displayed) {
        if (!invalidated) { pending.enrichments++; }
        pending.prepared = result ?? undefined;
        // VS Code requires a hidden widget before triggerSuggest. Reuse the
        // prepared result when it asks again, avoiding a second provider request.
        await vscode.commands.executeCommand("hideSuggestWidget");
        if (this.pending === pending && this.current(pending)) {
          await vscode.commands.executeCommand("editor.action.triggerSuggest");
        }
      } else if (!result || Array.isArray(result) || !result.isIncomplete) {
        if (items(result).length === 0 && pending.displayed !== "[]") {
          await vscode.commands.executeCommand("hideSuggestWidget");
        }
        if (this.pending === pending) { this.clear(); }
      }
    }).then(() => {
      cancellation.dispose();
      if (pending.cancellation === cancellation) { pending.cancellation = undefined; }
      pending.refreshing = false;
      if (this.pending === pending && pending.settled && pending.ready) { this.ready(pending.ready); }
    }, error => {
      cancellation.dispose();
      this.output.trace(`Completion refresh failed: ${String(error)}`);
      if (this.pending === pending) { this.clear(); }
    });
  }

  private matches(pending: PendingCompletion, ready: Pick<CompletionReady, "uri" | "version" | "position">): boolean {
    return pending.uri === ready.uri && pending.version === ready.version
      && pending.position.line === ready.position.line && pending.position.character === ready.position.character;
  }

  private current(pending: PendingCompletion): boolean {
    const editor = vscode.window.activeTextEditor;
    return !!editor && editor.document.uri.toString() === pending.uri && editor.document.version === pending.version
      && editor.selection.isEmpty && editor.selection.active.isEqual(pending.position)
      && vscode.window.state.focused;
  }

  public clear(): void {
    if (this.pending?.timer) { clearTimeout(this.pending.timer); }
    this.pending?.cancellation?.cancel();
    this.pending?.cancellation?.dispose();
    this.pending = undefined;
    void vscode.commands.executeCommand("setContext", "shucked.completionPending", false);
  }

  public dispose(): void { this.clear(); for (const subscription of this.subscriptions) { subscription.dispose(); } }
}
