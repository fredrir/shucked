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
import { resolveBinary } from "./binary";
import { StatusBarManager } from "./status";

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
  ) {}

  public error(
    error: Error,
    message: Message | undefined,
    count: number | undefined,
  ): ErrorHandlerResult {
    this.outputChannel.error(
      `Language server error (count: ${count ?? 1}): ${error.message}${
        message ? ` | message: ${JSON.stringify(message)}` : ""
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
 * Manages the lifecycle of the Shucked LanguageClient.
 */
export class ClientManager implements vscode.Disposable {
  private client: LanguageClient | undefined;
  private readonly crashTracker = new CrashTracker();
  private restartPromise: Promise<void> = Promise.resolve();
  private isRestarting = false;
  private manualShutdown = false;
  private traceChannel: vscode.LogOutputChannel | undefined;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly outputChannel: vscode.LogOutputChannel,
    private readonly statusManager: StatusBarManager,
  ) {}

  public get isRunning(): boolean {
    return this.client?.isRunning() ?? false;
  }

  public async start(): Promise<void> {
    this.manualShutdown = false;
    this.statusManager.setStatus("starting", "Resolving Shucked binary...");

    let binaryPath: string;
    try {
      binaryPath = await resolveBinary(this.context, this.outputChannel);
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      this.statusManager.setStatus("error", msg);
      this.outputChannel.error(`Failed to resolve binary: ${msg}`);
      this.reportStartFailure(msg);
      return;
    }

    const config = vscode.workspace.getConfiguration("shucked");
    const extraArgs = config.get<string[]>("server.extraArgs", []);

    const serverOptions: ServerOptions = {
      command: binaryPath,
      args: ["server", ...extraArgs],
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
      ],
      outputChannel: this.outputChannel,
      traceOutputChannel: this.traceChannel,
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

    this.client.onDidChangeState((event) => {
      switch (event.newState) {
        case State.Starting: {
          this.statusManager.setStatus("starting");
          break;
        }
        case State.Running: {
          this.statusManager.setStatus("ready");
          break;
        }
        case State.Stopped: {
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
    void this.stop();
  }
}
