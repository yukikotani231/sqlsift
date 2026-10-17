import { commands, ExtensionContext, OutputChannel, window, workspace } from "vscode";
import {
  DocumentFilter,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";
import * as path from "path";
import * as fs from "fs";

let client: LanguageClient | undefined;
let outputChannel: OutputChannel | undefined;
let restarting: Promise<void> = Promise.resolve();

function getBundledServerPath(context: ExtensionContext): string | undefined {
  const binaryName =
    process.platform === "win32" ? "sqlsift-lsp.exe" : "sqlsift-lsp";
  const bundledPath = path.join(context.extensionPath, "bin", binaryName);
  if (fs.existsSync(bundledPath)) {
    return bundledPath;
  }
  return undefined;
}

/**
 * Languages whose SQL is embedded in code: tagged template literals (sql`...`)
 * sqlx macros (query!("...")) and Python and Go database calls
 * (cursor.execute("..."), db.Query("..."))
 */
const EMBEDDED_SQL_LANGUAGES = [
  "typescript",
  "typescriptreact",
  "javascript",
  "javascriptreact",
  "rust",
  "python",
  "go",
];

/**
 * Documents sent to the server: SQL, dbt models (`jinja-sql` from dbt Power
 * User, or `.sql` files in the generic `jinja` language) and, when enabled,
 * TypeScript / JavaScript / Rust / Python / Go files with embedded SQL
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

function createClient(context: ExtensionContext): LanguageClient {
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
    outputChannel,
  };

  return new LanguageClient(
    "sqlsift",
    "sqlsift",
    serverOptions,
    clientOptions
  );
}

async function startServer(context: ExtensionContext): Promise<void> {
  if (client) {
    try {
      await client.stop();
    } catch {
      // Ignore stop errors if previous server was not running or crashed
    }
    client = undefined;
  }

  client = createClient(context);
  try {
    await client.start();
  } catch (err) {
    outputChannel?.appendLine(`Failed to start sqlsift language server: ${err}`);
  }
}

export function activate(context: ExtensionContext) {
  outputChannel = window.createOutputChannel("sqlsift");
  context.subscriptions.push(outputChannel);

  context.subscriptions.push(
    commands.registerCommand("sqlsift.restartServer", () => {
      restarting = restarting.then(() => startServer(context));
      return restarting;
    }),
    commands.registerCommand("sqlsift.showOutput", () => {
      outputChannel?.show();
    })
  );

  restarting = restarting.then(() => startServer(context));
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop().catch(() => {});
}
