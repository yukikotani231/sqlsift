import { ExtensionContext, workspace } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: ExtensionContext) {
  const config = workspace.getConfiguration("sqlsurge");
  const serverPath = config.get<string>("serverPath", "sqlsurge-lsp");

  const serverOptions: ServerOptions = {
    command: serverPath,
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "sql" }],
  };

  client = new LanguageClient(
    "sqlsurge",
    "sqlsurge",
    serverOptions,
    clientOptions
  );

  client.start();
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
