import * as vscode from "vscode";
import {
  CloseAction,
  CloseHandlerResult,
  ErrorAction,
  ErrorHandler,
  ErrorHandlerResult,
  LanguageClient,
  LanguageClientOptions,
  Message,
  ServerOptions,
  State,
} from "vscode-languageclient/node";
import { resolveServerCommand, ServerCommand } from "./binary";
import { StatusBarManager } from "./status";
import { CompletionRefresh } from "./completion-refresh";

/**
 * Tracks server crashes within a rolling time window to detect crash loops.
 */
export class CrashTracker {
  private readonly maxCrashes = 5;
  private readonly windowMs = 3 * 60 * 1000; // 3 minutes
  private crashTimestamps: number[] = [];

  public recordCrash(): { isCrashLoop: boolean; crashCount: number } {
    const now = Date.now();
    this.crashTimestamps = this.crashTimestamps.filter(
      (ts) => now - ts < this.windowMs,
    );
    this.crashTimestamps.push(now);
    return {
      isCrashLoop: this.crashTimestamps.length >= this.maxCrashes,
      crashCount: this.crashTimestamps.length,
    };
  }

  public reset(): void {
    this.crashTimestamps = [];
  }
}

/**
 * Custom ErrorHandler for the Shucked language server.
 * Handles crashes with exponential backoff and prevents crash loops.
 */
class ShuckedErrorHandler implements ErrorHandler {
  constructor(
    private readonly crashTracker: CrashTracker,
    private readonly outputChannel: vscode.LogOutputChannel,
    private readonly statusManager: StatusBarManager,
    private readonly onRestart: () => Promise<void>,
    private readonly isManualShutdown: () => boolean,
  ) { }

  public error(
    error: Error,
    message: Message | undefined,
    count: number | undefined,
  ): ErrorHandlerResult {
    this.outputChannel.error(
      `Language server error (count: ${count ?? 1}): ${error.message}${message ? ` | message: ${JSON.stringify(message)}` : ""
      }`,
    );

    if (count !== undefined && count <= 3) {
      return { action: ErrorAction.Continue, handled: true };
    }
    return { action: ErrorAction.Shutdown, handled: true };
  }

  public async closed(): Promise<CloseHandlerResult> {
    if (this.isManualShutdown()) {
      return { action: CloseAction.DoNotRestart, handled: true };
    }

    const { isCrashLoop, crashCount } = this.crashTracker.recordCrash();

    if (isCrashLoop) {
      this.statusManager.setStatus(
        "error",
        "Shucked language server crashed repeatedly.",
      );
      this.outputChannel.error(
        `Language server crashed ${crashCount} times within 3 minutes. Stopping automatic restarts.`,
      );

      void vscode.window
        .showErrorMessage(
          "Shucked language server crashed repeatedly and has stopped restarting.",
          "Restart Server",
          "Show Log",
        )
        .then((selected) => {
          if (selected === "Restart Server") {
            void this.onRestart();
          } else if (selected === "Show Log") {
            this.outputChannel.show(true);
          }
        });

      return {
        action: CloseAction.DoNotRestart,
        handled: true,
      };
    }

    const backoffMs = Math.min(500 * Math.pow(2, crashCount - 1), 8000);
    this.outputChannel.warn(
      `Language server connection closed (crash count: ${crashCount}). Restarting in ${backoffMs}ms...`,
    );
    this.statusManager.setStatus(
      "starting",
      `Restarting after crash (${crashCount})...`,
    );

    await new Promise((resolve) => setTimeout(resolve, backoffMs));

    return {
      action: CloseAction.Restart,
      handled: true,
    };
  }
}

interface ProgressNotificationParams {
  token: unknown;
  value: {
    kind?: "begin" | "report" | "end";
    title?: string;
    message?: string;
  };
}

/**
 * Constructs initialization options from the current `shucked` workspace configuration.
 */
export function getInitializationOptions(
  config: vscode.WorkspaceConfiguration,
): Record<string, unknown> {
  return {
    nativeExecutionAllowed: vscode.workspace.isTrusted,
    environment: config.get("environment"),
    unsafeFixes: config.get("unsafeFixes"),
    fixAll: config.get("fixAll"),
    lint: config.get("lint"),
    format: config.get("format"),
    codeAction: config.get("codeAction"),
    server: config.get("server"),
    tracing: getTracingOptions(config),
  };
}

/**
 * Server log settings (`shucked.trace.logLevel`, `shucked.trace.logFile`).
 * Only explicit values are forwarded so the server keeps its own defaults;
 * an empty log file path never creates a file.
 */
export function getTracingOptions(
  config: vscode.WorkspaceConfiguration,
): { logLevel?: string; logFile?: string } {
  const logLevel = config.get<string>("trace.logLevel");
  const logFile = config.get<string>("trace.logFile");
  return {
    ...(logLevel && ["error", "warn", "info", "debug", "trace"].includes(logLevel) ? { logLevel } : {}),
    ...(logFile?.trim() ? { logFile: logFile.trim() } : {}),
  };
}

/**
 * Manages the lifecycle of the Shucked LanguageClient.
 */
export class ClientManager implements vscode.Disposable {
  private client: LanguageClient | undefined;
  private readonly ready = new vscode.EventEmitter<void>();
  public readonly onReady = this.ready.event;
  private readonly crashTracker = new CrashTracker();
  private restartPromise: Promise<void> = Promise.resolve();
  private isRestarting = false;
  private manualShutdown = false;
  private readonly completionRefresh: CompletionRefresh;
  private traceChannel: vscode.LogOutputChannel | undefined;
  private readonly requestHandlers = new Map<string, { handler: (params: unknown, cancellation: vscode.CancellationToken) => Promise<unknown>; registration?: vscode.Disposable }>();

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly outputChannel: vscode.LogOutputChannel,
    private readonly statusManager: StatusBarManager,
  ) { this.completionRefresh = new CompletionRefresh(outputChannel); }

  public configurationOptions(): Record<string, unknown> {
    return {
      shucked: getInitializationOptions(vscode.workspace.getConfiguration("shucked")),
      workspace: Object.fromEntries((vscode.workspace.workspaceFolders ?? []).map(folder => [folder.uri.toString(), getInitializationOptions(vscode.workspace.getConfiguration("shucked", folder.uri))])),
    };
  }

  public async synchronizeConfiguration(): Promise<void> {
    await this.notify("workspace/didChangeConfiguration", { settings: this.configurationOptions() });
  }

  public onRequest(method: string, handler: (params: unknown, cancellation: vscode.CancellationToken) => Promise<unknown>): vscode.Disposable {
    const entry = { handler, registration: this.client?.onRequest(method, handler) };
    this.requestHandlers.set(method, entry);
    return { dispose: () => { entry.registration?.dispose(); if (this.requestHandlers.get(method) === entry) { this.requestHandlers.delete(method); } } };
  }

  public async notify(method: string, params: unknown): Promise<void> {
    if (this.client?.isRunning()) { await this.client.sendNotification(method, params); }
  }

  public async request<T>(method: string, params: unknown): Promise<T | undefined> {
    return this.client?.isRunning() ? this.client.sendRequest<T>(method, params) : undefined;
  }

  public get isRunning(): boolean {
    return this.client?.isRunning() ?? false;
  }

  public async start(): Promise<void> {
    this.manualShutdown = false;
    this.outputChannel.info(`Workspace host: ${vscode.env.remoteName ?? "local"}; ${process.platform}/${process.arch}`);
    this.statusManager.setStatus("starting", "Resolving Shucked binary...");

    const config = vscode.workspace.getConfiguration("shucked");
    const extraArgs = config.get<string[]>("server.extraArgs", []);

    let serverCmd: ServerCommand;
    try {
      serverCmd = await resolveServerCommand(this.context, this.outputChannel, extraArgs);
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      this.statusManager.setStatus("error", msg);
      this.outputChannel.error(`Failed to resolve server binary: ${msg}`);
      this.reportStartFailure(msg);
      return;
    }

    const serverOptions: ServerOptions = {
      command: serverCmd.command,
      args: serverCmd.args,
    };

    if (!this.traceChannel) {
      this.traceChannel = vscode.window.createOutputChannel("Shucked Language Server Trace", {
        log: true,
      });
      this.context.subscriptions.push(this.traceChannel);
    }

    const clientOptions: LanguageClientOptions = {
      documentSelector: [
        { scheme: "file", language: "shellscript" },
        { scheme: "untitled", language: "shellscript" },
        ...["bash", "zsh", "sh", "ksh", "fish"].flatMap(language => [{ scheme: "file", language }, { scheme: "untitled", language }]),
      ],
      outputChannel: this.outputChannel,
      traceOutputChannel: this.traceChannel,
      initializationOptions: this.configurationOptions(),
      middleware: {
        provideCompletionItem: (document, position, context, token, next) =>
          this.completionRefresh.provide(document, position, context, token, next),
      },
      errorHandler: new ShuckedErrorHandler(
        this.crashTracker,
        this.outputChannel,
        this.statusManager,
        () => this.restart(),
        () => this.manualShutdown,
      ),
    };

    this.client = new LanguageClient(
      "shucked",
      "Shucked Language Server",
      serverOptions,
      clientOptions,
    );

    for (const [method, entry] of this.requestHandlers) { entry.registration?.dispose(); entry.registration = this.client.onRequest(method, entry.handler); }
    const activeClient = this.client;
    activeClient.onNotification("shucked/completionReady", value => {
      if (this.client === activeClient) { this.completionRefresh.ready(value); }
    });
    activeClient.onDidChangeState((event) => {
      if (this.client !== activeClient) { return; }
      switch (event.newState) {
        case State.Starting: {
          this.completionRefresh.clear();
          this.statusManager.setStatus("starting");
          break;
        }
        case State.Running: {
          this.statusManager.setStatus("ready");
          this.ready.fire();
          break;
        }
        case State.Stopped: {
          this.completionRefresh.clear();
          this.statusManager.setStatus("stopped");
          break;
        }
      }
    });

    try {
      this.statusManager.setStatus("starting", "Starting language server...");
      await this.client.start();

      // Listen for work done progress notifications
      this.client.onNotification(
        "$/progress",
        (params: ProgressNotificationParams) => {
          if (!params?.value) {
            return;
          }
          const { kind, title, message } = params.value;
          if (kind === "begin") {
            this.statusManager.setStatus("busy", message || title || "Indexing...");
          } else if (kind === "end") {
            this.statusManager.setStatus("ready");
          } else if (kind === "report" && title) {
            this.statusManager.setStatus("busy", message || title);
          }
        },
      );

      this.outputChannel.info("Shucked language server started successfully.");
      this.statusManager.setStatus("ready");
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      this.statusManager.setStatus("error", msg);
      this.outputChannel.error(`Failed to start language server: ${msg}`);
      this.reportStartFailure(msg);
    }
  }

  public async stop(): Promise<void> {
    this.completionRefresh.clear();
    this.manualShutdown = true;
    const currentClient = this.client;
    this.client = undefined;

    if (currentClient) {
      try {
        if (currentClient.isRunning()) {
          await currentClient.stop();
        }
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        this.outputChannel.error(`Failed to stop language server: ${msg}`);
      }
    }
    this.statusManager.setStatus("stopped");
  }

  public restart(): Promise<void> {
    if (this.isRestarting) {
      return this.restartPromise;
    }

    this.isRestarting = true;
    this.restartPromise = (async () => {
      try {
        this.crashTracker.reset();
        this.outputChannel.info("Restarting Shucked language server...");
        await this.stop();
        await this.start();
      } finally {
        this.isRestarting = false;
      }
    })();

    return this.restartPromise;
  }

  private reportStartFailure(reason: string): void {
    void vscode.window
      .showErrorMessage(
        `Shucked language server failed to start: ${reason}`,
        "Retry",
        "Show Log",
      )
      .then((action) => {
        if (action === "Retry") {
          void this.restart();
        } else if (action === "Show Log") {
          this.outputChannel.show(true);
        }
      });
  }

  public dispose(): void {
    this.completionRefresh.dispose();
    this.ready.dispose();
    void this.stop();
  }
}
