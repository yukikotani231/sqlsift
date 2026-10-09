import { ExtensionContext, workspace } from "vscode";
import {
  DocumentFilter,
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

/** Languages whose SQL is in tagged template literals (sql`...`) */
const EMBEDDED_SQL_LANGUAGES = [
  "typescript",
  "typescriptreact",
  "javascript",
  "javascriptreact",
];

/**
 * Documents sent to the server: SQL, dbt models (`jinja-sql` from dbt Power
 * User, or `.sql` files in the generic `jinja` language) and, when enabled,
 * TypeScript / JavaScript files with embedded SQL
 */
function documentSelector(embeddedSql: boolean): DocumentFilter[] {
  const selector: DocumentFilter[] = [
    { scheme: "file", language: "sql" },
    { scheme: "file", language: "jinja-sql" },
    { scheme: "file", language: "jinja", pattern: "**/*.sql" },
  ];
  if (embeddedSql) {
    for (const language of EMBEDDED_SQL_LANGUAGES) {
      selector.push({ scheme: "file", language });
    }
  }
  return selector;
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
    documentSelector: documentSelector(
      config.get<boolean>("embeddedSql.enable", true)
    ),
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
