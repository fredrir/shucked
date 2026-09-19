import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import { binaryPlatform, hostTarget } from "../platform.mjs";


export function expandVariables(value: string): string {
  const home = value.replace(/^~(?=$|[/\\])/, os.homedir());
  return home.replace(
    /\$\{?([A-Za-z_]\w*)\}?/g,
    (match, name: string) => process.env[name] ?? match,
  );
}

export function ensureExecutable(filePath: string, repairPermissions = false): boolean {
  try {
    if (!fs.existsSync(filePath)) {
      return false;
    }
    const stat = fs.statSync(filePath);
    if (!stat.isFile()) {
      return false;
    }
    if (process.platform !== "win32") {
      try {
        fs.accessSync(filePath, fs.constants.X_OK);
      } catch {
        if (!repairPermissions) {
          return false;
        }
        try {
          fs.chmodSync(filePath, 0o755);
          fs.accessSync(filePath, fs.constants.X_OK);
        } catch {
          return false;
        }
      }
    }
    return true;
  } catch {
    return false;
  }
}

function bundledExecutable(filePath: string, output: vscode.LogOutputChannel): boolean {
  try {
    const binary = binaryPlatform(filePath);
    const metadataPath = path.join(path.dirname(filePath), "platform.json");
    const metadata: unknown = fs.existsSync(metadataPath)
      ? JSON.parse(fs.readFileSync(metadataPath, "utf8"))
      : undefined;
    const target = metadata && typeof metadata === "object" && "target" in metadata
      ? metadata.target
      : undefined;
    if (binary?.platform !== process.platform || binary?.arch !== process.arch || (target !== undefined && target !== hostTarget())) {
      output.warn(`Skipping incompatible bundled binary: ${filePath}. Host: ${hostTarget()}`);
      return false;
    }
    return ensureExecutable(filePath, true);
  } catch {
    return false;
  }
}

export function findInPath(exeName: string): string | undefined {
  const pathEnv = process.env.PATH;
  if (!pathEnv) {
    return undefined;
  }
  const extensions = process.platform === "win32"
    ? (process.env.PATHEXT?.split(";") ?? [".exe", ".cmd", ".bat"])
    : [""];

  const dirs = pathEnv.split(path.delimiter);
  for (const dir of dirs) {
    if (!dir) {
      continue;
    }
    for (const ext of extensions) {
      const candidate = path.join(
        dir,
        exeName.toLowerCase().endsWith(ext.toLowerCase()) && ext !== ""
          ? exeName
          : `${exeName}${ext}`,
      );
      if (ensureExecutable(candidate)) {
        return candidate;
      }
    }
  }
  return undefined;
}

export interface ServerCommand {
  command: string;
  args: string[];
}


export async function resolveBinary(
  context: vscode.ExtensionContext,
  outputChannel: vscode.LogOutputChannel,
  binaryName: "shucked" | "shucked-server" = "shucked",
): Promise<string> {
  const config = vscode.workspace.getConfiguration("shucked");
  const customPath = config.get<string>("server.path", "").trim();

  if (customPath.length > 0) {
    const expanded = expandVariables(customPath);
    outputChannel.warn(
      `Using custom binary path from 'shucked.server.path': "${expanded}". Custom binaries are unsupported and provided as-is.`,
    );
    if (ensureExecutable(expanded)) {
      return expanded;
    }
    throw new Error(
      `Configured 'shucked.server.path' is not an executable file: "${expanded}"`,
    );
  }

  const isWin = process.platform === "win32";
  const exeName = isWin ? `${binaryName}.exe` : binaryName;

  const bundledPath = path.join(context.extensionPath, "bin", exeName);
  if (bundledExecutable(bundledPath, outputChannel)) {
    outputChannel.info(`Found bundled binary: "${bundledPath}"`);
    return bundledPath;
  }

  const releaseArtifact = path.resolve(
    context.extensionPath,
    "../../target/release",
    exeName,
  );
  if (ensureExecutable(releaseArtifact)) {
    outputChannel.info(`Found workspace release build artifact: "${releaseArtifact}"`);
    return releaseArtifact;
  }

  const debugArtifact = path.resolve(
    context.extensionPath,
    "../../target/debug",
    exeName,
  );
  if (ensureExecutable(debugArtifact)) {
    outputChannel.info(`Found workspace debug build artifact: "${debugArtifact}"`);
    return debugArtifact;
  }

  const pathBinary = findInPath(exeName);
  if (pathBinary) {
    outputChannel.info(`Found binary in system PATH: "${pathBinary}"`);
    return pathBinary;
  }


  throw new Error(
    `Error: '${binaryName}' not found in PATH`,
  );
}


export async function resolveServerCommand(
  context: vscode.ExtensionContext,
  outputChannel: vscode.LogOutputChannel,
  extraArgs: string[] = [],
): Promise<ServerCommand> {
  const config = vscode.workspace.getConfiguration("shucked");
  const customPath = config.get<string>("server.path", "").trim();

  if (customPath.length > 0) {
    const expanded = expandVariables(customPath);
    outputChannel.warn(
      `Using custom binary path from 'shucked.server.path': "${expanded}". Custom binaries are unsupported and provided as-is.`,
    );
    if (ensureExecutable(expanded)) {
      const base = path.basename(expanded).toLowerCase();
      const isDedicated = base.startsWith("shucked-server");
      return {
        command: expanded,
        args: isDedicated ? [...extraArgs] : ["server", ...extraArgs],
      };
    }
    throw new Error(
      `Configured 'shucked.server.path' is not an executable file: "${expanded}"`,
    );
  }

  const isWin = process.platform === "win32";
  const serverExeName = isWin ? "shucked-server.exe" : "shucked-server";
  const cliExeName = isWin ? "shucked.exe" : "shucked";

  const bundledServer = path.join(context.extensionPath, "bin", serverExeName);
  if (bundledExecutable(bundledServer, outputChannel)) {
    outputChannel.info(`Found bundled language server binary: "${bundledServer}"`);
    return { command: bundledServer, args: [...extraArgs] };
  }

  const bundledCli = path.join(context.extensionPath, "bin", cliExeName);
  if (bundledExecutable(bundledCli, outputChannel)) {
    outputChannel.info(`Found bundled CLI binary: "${bundledCli}"`);
    return { command: bundledCli, args: ["server", ...extraArgs] };
  }

  const releaseServer = path.resolve(
    context.extensionPath,
    "../../target/release",
    serverExeName,
  );
  if (ensureExecutable(releaseServer)) {
    outputChannel.info(`Found workspace release build artifact: "${releaseServer}"`);
    return { command: releaseServer, args: [...extraArgs] };
  }

  const releaseCli = path.resolve(
    context.extensionPath,
    "../../target/release",
    cliExeName,
  );
  if (ensureExecutable(releaseCli)) {
    outputChannel.info(`Found workspace release build artifact: "${releaseCli}"`);
    return { command: releaseCli, args: ["server", ...extraArgs] };
  }

  const debugServer = path.resolve(
    context.extensionPath,
    "../../target/debug",
    serverExeName,
  );
  if (ensureExecutable(debugServer)) {
    outputChannel.info(`Found workspace debug build artifact: "${debugServer}"`);
    return { command: debugServer, args: [...extraArgs] };
  }

  const debugCli = path.resolve(
    context.extensionPath,
    "../../target/debug",
    cliExeName,
  );

  if (ensureExecutable(debugCli)) {
    outputChannel.info(`Found workspace debug build artifact: "${debugCli}"`);
    return { command: debugCli, args: ["server", ...extraArgs] };
  }

  const pathServer = findInPath(serverExeName);
  if (pathServer) {
    outputChannel.info(`Found language server binary in system PATH: "${pathServer}"`);
    return { command: pathServer, args: [...extraArgs] };
  }

  const pathCli = findInPath(cliExeName);
  if (pathCli) {
    outputChannel.info(`Found CLI binary in system PATH: "${pathCli}"`);
    return { command: pathCli, args: ["server", ...extraArgs] };
  }

  throw new Error(
    `Could not find a valid 'shucked-server' or 'shucked' binary. Please ensure 'shucked' or 'shucked-server' is installed and available in PATH, or set 'shucked.server.path'.`,
  );
}
