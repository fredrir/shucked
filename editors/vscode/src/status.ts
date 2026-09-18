import * as vscode from "vscode";

export type ServerStatus = "starting" | "ready" | "busy" | "error" | "stopped";

/**
 * Manages the Shucked status bar item in VS Code.
 */
export class StatusBarManager implements vscode.Disposable {
  private readonly statusBarItem: vscode.StatusBarItem;
  private status: ServerStatus = "starting";

  constructor() {
    this.statusBarItem = vscode.window.createStatusBarItem(
      vscode.StatusBarAlignment.Right,
      100,
    );
    this.statusBarItem.command = "shucked.statusClicked";
    this.setStatus("starting");
    this.statusBarItem.show();
  }

  public get currentStatus(): ServerStatus {
    return this.status;
  }

  public setStatus(status: ServerStatus, detail?: string): void {
    this.status = status;
    switch (status) {
      case "starting": {
        this.statusBarItem.text = "$(sync~spin) Shucked: Starting";
        this.statusBarItem.tooltip = detail ?? "Shucked language server is starting...";
        break;
      }
      case "ready": {
        this.statusBarItem.text = "$(check) Shucked";
        this.statusBarItem.tooltip = detail ?? "Shucked language server is ready";
        break;
      }
      case "busy": {
        this.statusBarItem.text = "$(sync~spin) Shucked: Indexing";
        this.statusBarItem.tooltip = detail ?? "Shucked language server is indexing...";
        break;
      }
      case "error": {
        this.statusBarItem.text = "$(error) Shucked: Error";
        this.statusBarItem.tooltip =
          detail ?? "Shucked language server encountered an error. Click to restart or view logs.";
        break;
      }
      case "stopped": {
        this.statusBarItem.text = "$(circle-slash) Shucked: Stopped";
        this.statusBarItem.tooltip =
          detail ?? "Shucked language server stopped. Click to restart or view logs.";
        break;
      }
    }
  }

  public dispose(): void {
    this.statusBarItem.dispose();
  }
}
