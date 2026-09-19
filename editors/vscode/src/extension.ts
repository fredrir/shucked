import * as vscode from "vscode";
import { ClientManager } from "./client";
import { registerCommands } from "./commands";
import { registerConfigWatcher } from "./config";
import { StatusBarManager } from "./status";

let clientManager: ClientManager | undefined;
let outputChannel: vscode.LogOutputChannel | undefined;

/**
 * Extension activation entry point.
 */
export async function activate(context: vscode.ExtensionContext): Promise<void> {
  outputChannel = vscode.window.createOutputChannel("Shucked", { log: true });
  context.subscriptions.push(outputChannel);
  outputChannel.info("Shucked extension activating...");

  const statusManager = new StatusBarManager();
  context.subscriptions.push(statusManager);

  clientManager = new ClientManager(context, outputChannel, statusManager);
  context.subscriptions.push(clientManager);

  registerCommands(context, clientManager, outputChannel, statusManager);
  registerConfigWatcher(context, clientManager, outputChannel);
  context.subscriptions.push(
    vscode.workspace.onDidGrantWorkspaceTrust(() => {
      void clientManager?.restart();
    }),
  );

  // Multi-root workspace change handling
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders((event) => {
      outputChannel?.info(
        `Workspace folders changed. Added: ${event.added.length}, Removed: ${event.removed.length}`,
      );
    }),
  );

  await clientManager.start();
  outputChannel.info("Shucked extension activation complete.");
}

/**
 * Extension deactivation entry point.
 */
export async function deactivate(): Promise<void> {
  if (clientManager) {
    await clientManager.stop();
    clientManager = undefined;
  }
}