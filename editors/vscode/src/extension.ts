import { ExtensionContext, workspace } from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export function activate(context: ExtensionContext) {
  const config = workspace.getConfiguration("sqlsift");
  const serverPath = config.get<string>("serverPath", "sqlsift-lsp");

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
