import { ExtensionContext, workspace } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";
import * as path from "path";
import * as fs from "fs";

let client: LanguageClient | undefined;

function getBundledServerPath(context: ExtensionContext): string | undefined {
  const binaryName =
    process.platform === "win32" ? "sqlsift-lsp.exe" : "sqlsift-lsp";
  const bundledPath = path.join(context.extensionPath, "bin", binaryName);
  if (fs.existsSync(bundledPath)) {
    return bundledPath;
  }
  return undefined;
}

export function activate(context: ExtensionContext) {
  const config = workspace.getConfiguration("sqlsift");
  const configuredPath = config.get<string>("serverPath", "sqlsift-lsp");

  // Priority:
  // 1. User explicitly set serverPath → use that
  // 2. Bundled binary exists → use bundled
  // 3. Fallback → PATH lookup
  const bundledPath = getBundledServerPath(context);
  const serverPath =
    configuredPath !== "sqlsift-lsp"
      ? configuredPath
      : bundledPath ?? configuredPath;

  const serverOptions: ServerOptions = {
    command: serverPath,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "sql" }],
  };

  client = new LanguageClient(
    "sqlsift",
    "sqlsift",
    serverOptions,
    clientOptions
  );

  client.start();
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
