// tests/api/deep_health.rs

use crate::helpers::spawn_app;

#[tokio::test]
async fn deep_health_reports_all_dependencies_ok() {
    // Arrange — the local test configuration uses the in-memory storage
    // backend and the harness's freshly-migrated database, so every
    // dependency should report healthy.
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .get(&format!("{}/api/v1/health", &app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert!(response.status().is_success());
    let report: serde_json::Value = response.json().await.expect("Expected a JSON body");
    assert_eq!(report["status"], "ok");
    assert_eq!(report["database"], "ok");
    assert_eq!(report["storage"], "ok");
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
}
