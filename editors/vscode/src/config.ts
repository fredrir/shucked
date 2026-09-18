import * as vscode from "vscode";
import { ClientManager } from "./client";

/**
 * Watches configuration changes for `shucked.*` settings.
 * Automatically restarts the language client when server binary or arguments change.
 */
export function registerConfigWatcher(
  context: vscode.ExtensionContext,
  clientManager: ClientManager,
  outputChannel: vscode.LogOutputChannel,
): void {
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (
        event.affectsConfiguration("shucked.server.path") ||
        event.affectsConfiguration("shucked.server.extraArgs")
      ) {
        outputChannel.info(
          "Configuration change detected for 'shucked.server'. Restarting language server...",
        );
        await clientManager.restart();
      }
    }),
  );
}
