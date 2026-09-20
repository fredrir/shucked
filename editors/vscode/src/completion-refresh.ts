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
  invalidations: number;
  responses: number;
  ready?: CompletionReady;
  settled: boolean;
  refreshing: boolean;
  timer?: ReturnType<typeof setTimeout>;
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
      generations: new Set(), enrichments: 0, invalidations: 0, responses: 0, settled: false, refreshing: false,
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
    pending.settled = false;
    const started = performance.now();
    const cancellation = token.onCancellationRequested(() => {
      if (this.pending === pending && !pending.settled && !pending.refreshing) { this.clear(); }
    });
    try {
      const result = await next(document, position, context, token);
      this.output.trace(`Completion response: ${Math.round(performance.now() - started)}ms, ${Array.isArray(result) ? result.length : result?.items.length ?? 0} candidates`);
      if (this.pending === pending) {
        pending.settled = true;
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
      || (invalidated ? pending.invalidations >= 8 : pending.enrichments >= 3)) { return; }
    if (!pending.settled || pending.refreshing) { pending.ready = value; return; }
    pending.ready = undefined;
    pending.generations.add(value.generation);
    if (invalidated) { pending.invalidations++; } else { pending.enrichments++; }
    pending.refreshing = true;
    this.output.trace(`Completion ready: ${value.provider ?? value.reason ?? "provider"}, ${value.elapsedMs ?? "?"}ms, ${value.candidateCount ?? "?"} candidates`);
    // VS Code requires the suggestion widget to be hidden before triggerSuggest.
    void vscode.commands.executeCommand("hideSuggestWidget").then(async () => {
      if (this.pending === pending && this.current(pending)) {
        await vscode.commands.executeCommand("editor.action.triggerSuggest");
      }
    }).then(() => {
      pending.refreshing = false;
      if (this.pending === pending && pending.settled && pending.ready) { this.ready(pending.ready); }
    }, error => {
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
    this.pending = undefined;
    void vscode.commands.executeCommand("setContext", "shucked.completionPending", false);
  }

  public dispose(): void { this.clear(); for (const subscription of this.subscriptions) { subscription.dispose(); } }
}
