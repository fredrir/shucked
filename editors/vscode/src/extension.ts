import * as os from "node:os";

import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

type ShellLanguage = "sh" | "bash" | "zsh";

const outputChannel = vscode.window.createOutputChannel("Shuck", { log: true });

let client: LanguageClient | undefined;
let restartPromise: Promise<void> = Promise.resolve();
let restartQueued = false;

function toMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function expandVariables(value: string): string {
  const home = value.replace(/^~(?=$|[/\\])/, os.homedir());
  return home.replace(
    /\$\{?([A-Za-z_]\w*)\}?/g,
    (match, name: string) => process.env[name] ?? match,
  );
}

async function startClient(): Promise<void> {
  const config = vscode.workspace.getConfiguration("shuck");

  const command = expandVariables(config.get<string>("path", "shuck"));
  const enabledShells = config.get<ShellLanguage[]>("enabledShells", [
    "sh",
    "bash",
    "zsh",
  ]);

  const serverOptions: ServerOptions = {
    command,
    args: ["server"],
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      {
        scheme: "file",
        language: "shellscript",
      },
      {
        scheme: "untitled",
        language: "shellscript",
      },
    ],

    initializationOptions: {
      enabledShells,
    },

    outputChannel,
  };

  client = new LanguageClient(
    "shuck",
    "Shuck Language Server",
    serverOptions,
    clientOptions,
  );

  await client.start();
}

async function stopClient(): Promise<void> {
  const currentClient = client;
  client = undefined;

  try {
    await currentClient?.stop();
  } catch (error) {
    outputChannel.error(`failed to stop language server: ${toMessage(error)}`);
  }
}

function reportStartFailure(error: unknown): void {
  const reason = toMessage(error);
  outputChannel.error(`language server failed to start: ${reason}`);

  void vscode.window
    .showErrorMessage(
      `Shuck language server failed to start: ${reason}`,
      "Retry",
      "Show Log",
    )
    .then((action) => {
      if (action === "Retry") {
        void restartClient();
      }

      if (action === "Show Log") {
        outputChannel.show();
      }
    });
}

function restartClient(): Promise<void> {
  if (restartQueued) {
    return restartPromise;
  }

  restartQueued = true;
  restartPromise = restartPromise.then(async () => {
    restartQueued = false;
    await stopClient();

    try {
      await startClient();
    } catch (error) {
      reportStartFailure(error);
    }
  });

  return restartPromise;
}

export async function activate(context: vscode.ExtensionContext) {
  context.subscriptions.push(
    outputChannel,
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (
        event.affectsConfiguration("shuck.path") ||
        event.affectsConfiguration("shuck.enabledShells")
      ) {
        void restartClient();
      }
    }),
  );

  await restartClient();
}

export async function deactivate() {
  await restartPromise;
  await stopClient();
}