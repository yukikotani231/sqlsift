mod config;
mod diagnostics;
mod server;
mod state;

use tower_lsp::{LspService, Server};
use tracing_subscriber::EnvFilter;

use crate::server::Backend;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("sqlsift_lsp=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
