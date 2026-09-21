// src/bin/main.rs

// dependencies
use halation::configuration::get_configuration;
use halation::startup::Application;
use halation::telemetry::{get_subscriber, init_subscriber};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present (dev convenience; silently skipped in production
    // containers where configuration arrives through real env vars).
    let _ = dotenvy::dotenv();

    let subscriber = get_subscriber("halation".into(), "info".into(), std::io::stdout);
    init_subscriber(subscriber);
    let configuration = get_configuration().expect("Failed to read configuration.");
    let application = Application::build(configuration.clone()).await?;
    application.run_until_stopped().await?;

    Ok(())
}
