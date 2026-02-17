// forwarder: Reads from IPICO timing hardware and forwards events to the server.
//
// Each task (5-8) extends this file with additional subsystem initialization.

use tracing::info;

fn main() {
    // Initialize tracing subscriber for structured logging to stdout.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "forwarder starting"
    );

    // Load config from default path.
    // Later tasks will use the loaded config to initialize subsystems.
    let _cfg = match forwarder::config::load_config() {
        Ok(cfg) => {
            info!(
                base_url = %cfg.server.base_url,
                readers = cfg.readers.len(),
                "config loaded"
            );
            cfg
        }
        Err(e) => {
            eprintln!("FATAL: failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Task 6+: init SQLite journal, uplink, fanout, status HTTP.
    info!("forwarder initialized (stub — subsystems added in later tasks)");
}
