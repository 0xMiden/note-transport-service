//! Process shutdown signalling.

use tracing::info;

/// Resolves when the process is asked to terminate.
///
/// Listens for `SIGTERM` — what Kubernetes and `docker stop` send — and for `SIGINT` via
/// `Ctrl-C`, so an operator gets the same drain in both cases. On non-Unix targets only
/// `Ctrl-C` is available.
///
/// # Panics
/// Panics if the signal handlers cannot be installed, which means the process cannot be
/// terminated cleanly and should not pretend otherwise.
pub async fn signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install the SIGTERM handler");
        let mut sigint =
            signal(SignalKind::interrupt()).expect("failed to install the SIGINT handler");

        let received = tokio::select! {
            _ = sigterm.recv() => "SIGTERM",
            _ = sigint.recv() => "SIGINT",
        };

        info!(signal = received, "Shutdown signal received");
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.expect("failed to install the Ctrl-C handler");
        info!(signal = "Ctrl-C", "Shutdown signal received");
    }
}
