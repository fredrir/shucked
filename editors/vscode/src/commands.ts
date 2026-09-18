import * as cp from "node:child_process";
import * as util from "node:util";
import * as vscode from "vscode";
import { resolveBinary } from "./binary";
import { ClientManager } from "./client";
import { StatusBarManager } from "./status";

const execFile = util.promisify(cp.execFile);

/**
 * Registers all Shucked extension commands.
 */
export function registerCommands(
  context: vscode.ExtensionContext,
  clientManager: ClientManager,
  outputChannel: vscode.LogOutputChannel,
  statusManager: StatusBarManager,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("shucked.restartServer", async () => {
      outputChannel.info("Command 'shucked.restartServer' invoked.");
      await clientManager.restart();
    }),

    vscode.commands.registerCommand("shucked.showOutputChannel", () => {
      outputChannel.show(true);
    }),

    vscode.commands.registerCommand("shucked.showVersion", async () => {
      try {
        const binPath = await resolveBinary(context, outputChannel);
        const { stdout, stderr } = await execFile(binPath, ["--version"]);
        const versionStr = (stdout || stderr).trim();
        outputChannel.info(`Shucked version: ${versionStr} (${binPath})`);
        void vscode.window.showInformationMessage(`Shucked: ${versionStr}`);
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        outputChannel.error(`Failed to retrieve Shucked version: ${msg}`);
        void vscode.window.showErrorMessage(`Failed to retrieve Shucked version: ${msg}`);
      }
    }),

    vscode.commands.registerCommand("shucked.statusClicked", async () => {
      if (statusManager.currentStatus === "error") {
        const choice = await vscode.window.showErrorMessage(
          "Shucked language server is in an error state.",
          "Restart Server",
          "Show Logs",
        );
        if (choice === "Restart Server") {
          await clientManager.restart();
        } else if (choice === "Show Logs") {
          outputChannel.show(true);
        }
      } else {
        outputChannel.show(true);
      }
    }),
  );
}
