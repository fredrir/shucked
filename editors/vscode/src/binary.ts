import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";

/**
 * Expands leading `~` to user's home directory and replaces `$VAR` / `${VAR}`
 * with environment variable values.
 */
export function expandVariables(value: string): string {
  const home = value.replace(/^~(?=$|[/\\])/, os.homedir());
  return home.replace(
    /\$\{?([A-Za-z_]\w*)\}?/g,
    (match, name: string) => process.env[name] ?? match,
  );
}

/**
 * Validates that the specified file exists, is a regular file, and has execute permissions.
 * On POSIX systems, attempts to `chmod 0o755` if execute permission is initially missing.
 */
export function ensureExecutable(filePath: string): boolean {
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

/**
 * Searches system PATH for the given executable name.
 */
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

/**
 * Resolves the `shucked` executable using the defined priority order:
 * 1. User-configured `shucked.server.path`
 * 2. Bundled platform binary in `bin/shucked`
 * 3. Local workspace build artifacts (`../../target/release/shucked`, `../../target/debug/shucked`)
 * 4. System PATH
 */
export async function resolveBinary(
  context: vscode.ExtensionContext,
  outputChannel: vscode.LogOutputChannel,
): Promise<string> {
  const config = vscode.workspace.getConfiguration("shucked");
  const customPath = config.get<string>("server.path", "").trim();

  // 1. User-configured binary path
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

  const exeName = process.platform === "win32" ? "shucked.exe" : "shucked";

  // 2. Bundled platform binary in extension directory: bin/shucked (or bin/shucked.exe)
  const bundledPath = path.join(context.extensionPath, "bin", exeName);
  if (ensureExecutable(bundledPath)) {
    outputChannel.info(`Found bundled binary: "${bundledPath}"`);
    return bundledPath;
  }

  // 3. Local workspace build artifacts for local development
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

  // Fallback to 'shuck' binary in target during development if present
  const altExeName = process.platform === "win32" ? "shuck.exe" : "shuck";
  const altReleaseArtifact = path.resolve(
    context.extensionPath,
    "../../target/release",
    altExeName,
  );
  if (ensureExecutable(altReleaseArtifact)) {
    outputChannel.info(
      `Found workspace release build artifact (shuck): "${altReleaseArtifact}"`,
    );
    return altReleaseArtifact;
  }

  const altDebugArtifact = path.resolve(
    context.extensionPath,
    "../../target/debug",
    altExeName,
  );
  if (ensureExecutable(altDebugArtifact)) {
    outputChannel.info(
      `Found workspace debug build artifact (shuck): "${altDebugArtifact}"`,
    );
    return altDebugArtifact;
  }

  // 4. System PATH
  const pathBinary = findInPath(exeName);
  if (pathBinary) {
    outputChannel.info(`Found binary in system PATH: "${pathBinary}"`);
    return pathBinary;
  }

  const altPathBinary = findInPath(altExeName);
  if (altPathBinary) {
    outputChannel.info(`Found fallback binary in system PATH: "${altPathBinary}"`);
    return altPathBinary;
  }

  throw new Error(
    `Could not find a valid '${exeName}' binary. Please ensure 'shucked' is installed and available in PATH, or set 'shucked.server.path'.`,
  );
}
