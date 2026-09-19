import * as vscode from "vscode";
import * as cp from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { promisify } from "node:util";
import { resolveBinary } from "./binary";

const execFile = promisify(cp.execFile);
const cell = (value: unknown): string => String(value ?? "").replace(/[\r\n|]/g, " ").replace(/[<>`]/g, "");

export function registerTargetCommands(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): void {
  const reports = new Map<string, string>();
  let sequence = 0;
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider("shucked-targets", { provideTextDocumentContent: uri => reports.get(uri.toString()) ?? "Report expired" }),
    vscode.workspace.onDidCloseTextDocument(document => { if (document.uri.scheme === "shucked-targets") { reports.delete(document.uri.toString()); } }),
    vscode.commands.registerCommand("shucked.captureTarget", async () => {
      const destination = await vscode.window.showSaveDialog({ title: vscode.workspace.isTrusted ? "Capture workspace host and supported tool capabilities" : "Capture workspace host availability", filters: { "Target inventory": ["json"] }, saveLabel: "Capture" });
      if (!destination) { return; }
      const shell = await vscode.window.showQuickPick(["bash", "zsh", "fish", "sh", "ksh"], { title: "Target interpreter" });
      if (!shell) { return; }
      const label = await vscode.window.showInputBox({ title: "Target label", value: vscode.env.remoteName ?? os.hostname(), validateInput: text => text.trim() ? undefined : "Enter a target label" });
      if (!label) { return; }
      try {
        const binary = await resolveBinary(context, output);
        const capabilities = vscode.workspace.isTrusted ? ["--capabilities"] : [];
        await execFile(binary, ["target", "capture", ...capabilities, "--label", label, "--shell", shell, "--output", destination.fsPath], { timeout: 30000, maxBuffer: 1024 * 1024 });
        void vscode.window.showInformationMessage(vscode.workspace.isTrusted ? "Target availability and supported tool capabilities captured." : "Target availability captured without running tool queries.");
      } catch (error) { void vscode.window.showErrorMessage(`Target capture failed: ${error instanceof Error ? error.message : String(error)}`); }
    }),
    vscode.commands.registerCommand("shucked.compareTargets", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor || !["shellscript", "bash", "zsh", "fish", "sh", "ksh"].includes(editor.document.languageId)) { return; }
      const targets = await vscode.window.showOpenDialog({ canSelectMany: true, title: "Compare captured targets", filters: { "Target inventories": ["json"] } });
      if (!targets?.length) { return; }
      let temporary: string | undefined;
      try {
        const binary = await resolveBinary(context, output);
        temporary = await fs.mkdtemp(path.join(os.tmpdir(), "shucked-compare-"));
        const extension = editor.document.languageId === "fish" ? ".fish" : path.extname(editor.document.fileName) || ".sh";
        const script = path.join(temporary, `document${extension}`);
        await fs.writeFile(script, editor.document.getText(), { mode: 0o600 });
        const { stdout } = await execFile(binary, ["target", "compare", ...targets.flatMap(target => ["--target", target.fsPath]), script], { timeout: 15000, maxBuffer: 16 * 1024 * 1024 });
        const report = JSON.parse(stdout) as Comparison;
        const columns = report.comparison.targets;
        const table = [
          `# Execution targets: ${cell(path.basename(editor.document.fileName))}`,
          "", "Recorded inventories; no local tool lookup is used for captured targets.", "", `Source: [${cell(path.basename(editor.document.fileName))}](${editor.document.uri.toString()})`, "",
          `| Command | Source line | ${columns.map(target => cell(target.label)).join(" | ")} |`,
          `| --- | --- | ${columns.map(() => "---").join(" | ")} |`,
          ...report.comparison.commands.map((command, index) => {
            const line = report.locations[index]?.line;
            const source = line ? `[${line}](${editor.document.uri.with({ fragment: `L${line}` }).toString()})` : "";
            const results = command.results.map((result, targetIndex) => {
              const version = result.command?.executable?.version;
              const validation = command.validation[targetIndex];
              const invalid = validation?.state === "invalid" && Array.isArray(validation.detail) ? validation.detail.map(issue => issue.value).join(", ") : "";
              return cell(`${result.state}${version ? ` (${version})` : ""}; arguments ${validation?.state ?? "unknown"}${invalid ? `: ${invalid}` : ""}`);
            });
            return `| ${cell(command.name ?? "dynamic")} | ${source} | ${results.join(" | ")} |`;
          }),
          "", "| Target | Platform | Captured | Inventory |", "| --- | --- | --- | --- |",
          ...columns.map(target => `| ${cell(target.label)} | ${cell(target.platform)} | ${cell(new Date(target.capturedUnixMs).toISOString())} | ${target.complete ? "Complete" : "Partial"} |`),
        ].join("\n");
        const uri = vscode.Uri.parse(`shucked-targets:/comparison-${++sequence}.md`);
        reports.set(uri.toString(), table);
        while (reports.size > 16) { const oldest = reports.keys().next().value; if (oldest) { reports.delete(oldest); } }
        await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(uri), { preview: true });
      } catch (error) { void vscode.window.showErrorMessage(`Target comparison failed: ${error instanceof Error ? error.message : String(error)}`); }
      finally { if (temporary) { await fs.rm(temporary, { recursive: true, force: true }); } }
    }),
  );
}
interface Comparison {
  comparison: { targets: { label: string; platform: string; capturedUnixMs: number; complete: boolean }[]; commands: { name?: string; results: { state: string; command?: { executable?: { version?: string } } }[]; validation: { state: string; detail?: string | { value: string }[] }[] }[] };
  locations: { line: number; column: number }[];
}
