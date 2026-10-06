//! The `database-settings` fields beyond `required_extensions` (ADR-095)
//! against a real store: their defaults, the typed update's write, clear and
//! rejection, the routing verdicts' lifetime, and a retyped singleton.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::{
    CaptureContent, DatabaseSettingsNodeUpdate, NodeUpdate, ProviderConfig,
};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{NodeService, NodeServiceError};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const SETTINGS_ID: &str = "database-settings-singleton";
const PROVIDER_ID: &str = "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d";

async fn service() -> (Arc<NodeService>, TempDir) {
    let dir = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(dir.path().join("test.db")).await.unwrap());
    let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
    (node_service, dir)
}

fn provider(base_url: &str, model: &str) -> ProviderConfig {
    ProviderConfig {
        id: PROVIDER_ID.to_string(),
        name: "Local".to_string(),
        base_url: base_url.to_string(),
        api_key: "secret".to_string(),
        model: model.to_string(),
        routing_ok: [("m1".to_string(), false)].into(),
    }
}

async fn write(
    svc: &NodeService,
    update: DatabaseSettingsNodeUpdate,
) -> Result<(), NodeServiceError> {
    let (_, version) = svc.database_settings().await.unwrap();
    svc.update_database_settings_node(SETTINGS_ID, version, update)
        .await
        .map(|_| ())
}

#[tokio::test]
async fn a_new_database_starts_from_the_schema_defaults() {
    let (svc, _dir) = service().await;

    let (settings, _) = svc.database_settings().await.unwrap();

    assert!(!settings.capture_enabled);
    assert_eq!(settings.capture_content, CaptureContent::MetadataOnly);
    assert!(settings.providers.is_empty());
    assert!(settings.required_extensions.is_empty());
}

#[tokio::test]
async fn the_update_writes_each_field_and_clearing_restores_its_default() {
    let (svc, _dir) = service().await;

    write(
        &svc,
        DatabaseSettingsNodeUpdate {
            capture_enabled: Some(Some(true)),
            capture_content: Some(Some(CaptureContent::Full)),
            providers: Some(Some(vec![provider("http://a/v1", "m")])),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (settings, _) = svc.database_settings().await.unwrap();
    assert!(settings.capture_enabled);
    assert_eq!(settings.capture_content, CaptureContent::Full);
    assert_eq!(settings.provider(PROVIDER_ID).unwrap().api_key, "secret");

    let stored = svc.get_node(SETTINGS_ID).await.unwrap().unwrap();
    assert_eq!(
        stored.properties["database-settings"]["capture_content"],
        json!("full")
    );

    write(
        &svc,
        DatabaseSettingsNodeUpdate {
            capture_enabled: Some(None),
            capture_content: Some(None),
            providers: Some(None),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (cleared, _) = svc.database_settings().await.unwrap();
    assert_eq!(
        cleared,
        nodespace_core::models::DatabaseSettingsFields::default()
    );
}

#[tokio::test]
async fn an_unchanged_update_conflicts_and_an_empty_one_is_refused() {
    let (svc, _dir) = service().await;
    let (_, version) = svc.database_settings().await.unwrap();
    let update = || DatabaseSettingsNodeUpdate {
        capture_enabled: Some(Some(true)),
        ..Default::default()
    };

    svc.update_database_settings_node(SETTINGS_ID, version, update())
        .await
        .unwrap();
    assert!(svc
        .update_database_settings_node(SETTINGS_ID, version, update())
        .await
        .is_err());
    assert!(svc
        .update_database_settings_node(
            SETTINGS_ID,
            version + 1,
            DatabaseSettingsNodeUpdate::default()
        )
        .await
        .is_err());
}

#[tokio::test]
async fn a_flat_update_is_validated_like_the_typed_one() {
    let (svc, _dir) = service().await;

    for bad in [
        json!({ "capture_enabled": "yes" }),
        json!({ "capture_content": "everything" }),
        json!({ "providers": "none" }),
        json!({ "providers": [{ "name": "n", "base_url": "u" }] }),
        json!({ "providers": [{ "id": PROVIDER_ID, "name": "n", "base_url": "u", "extra": 1 }] }),
        json!({ "providers": [{ "id": "not-a-uuid", "name": "n", "base_url": "u" }] }),
        json!({ "providers": [{ "id": PROVIDER_ID, "name": "n" }] }),
    ] {
        let settings = svc.get_node(SETTINGS_ID).await.unwrap().unwrap();
        let result = svc
            .update_node(
                SETTINGS_ID,
                settings.version,
                NodeUpdate::new().with_properties(bad.clone()),
            )
            .await;
        assert!(result.is_err(), "{bad} must be rejected");
    }
    assert_eq!(
        svc.database_settings().await.unwrap().0,
        nodespace_core::models::DatabaseSettingsFields::default()
    );
}

/// External tools are enabled by the `nodespace mcp install` client config,
/// not by a database setting, so the field is an undeclared key.
#[tokio::test]
async fn a_flat_update_naming_external_tools_enabled_is_rejected_as_undeclared() {
    let (svc, _dir) = service().await;
    let settings = svc.get_node(SETTINGS_ID).await.unwrap().unwrap();
    let err = svc
        .update_node(
            SETTINGS_ID,
            settings.version,
            NodeUpdate::new().with_properties(json!({ "external_tools_enabled": true })),
        )
        .await
        .expect_err("the removed field must be rejected");
    assert!(
        err.to_string().contains("external_tools_enabled"),
        "the error must name the key: {err}"
    );
}

#[tokio::test]
async fn routing_verdicts_are_dropped_when_the_endpoint_or_model_changes() {
    let (svc, _dir) = service().await;
    let set = |p: ProviderConfig| DatabaseSettingsNodeUpdate {
        providers: Some(Some(vec![p])),
        ..Default::default()
    };

    // A new config carries no verdicts, whatever the client sent.
    write(&svc, set(provider("http://a/v1", "m")))
        .await
        .unwrap();
    assert!(svc
        .database_settings()
        .await
        .unwrap()
        .0
        .provider(PROVIDER_ID)
        .unwrap()
        .routing_ok
        .is_empty());

    // The same endpoint and model keep a recorded verdict.
    write(&svc, set(provider("http://a/v1", "m")))
        .await
        .unwrap();
    assert!(!svc.database_settings().await.unwrap().0.providers[0].routing_ok["m1"]);

    // A changed model or endpoint drops it.
    for changed in [
        provider("http://a/v1", "other"),
        provider("http://b/v1", "m"),
    ] {
        write(&svc, set(provider("http://a/v1", "m")))
            .await
            .unwrap();
        write(&svc, set(changed)).await.unwrap();
        assert!(svc.database_settings().await.unwrap().0.providers[0]
            .routing_ok
            .is_empty());
    }
}

#[tokio::test]
async fn a_retyped_singleton_keeps_its_base_bucket_settings_readable_and_writable() {
    let (svc, _dir) = service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Fixture Settings",
            "extends": "database-settings",
            "fields": [
                { "name": "fixture_flag", "type": "boolean", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .unwrap();
    write(
        &svc,
        DatabaseSettingsNodeUpdate {
            capture_content: Some(Some(CaptureContent::Full)),
            providers: Some(Some(vec![provider("http://a/v1", "m")])),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let settings = svc.get_node(SETTINGS_ID).await.unwrap().unwrap();
    svc.update_node(
        SETTINGS_ID,
        settings.version,
        NodeUpdate {
            node_type: Some("fixture-settings".to_string()),
            properties: Some(json!({ "fixture_flag": true })),
            ..NodeUpdate::new()
        },
    )
    .await
    .unwrap();

    let (read, _) = svc.database_settings().await.unwrap();
    assert_eq!(read.capture_content, CaptureContent::Full);
    assert_eq!(read.providers.len(), 1);

    write(
        &svc,
        DatabaseSettingsNodeUpdate {
            capture_enabled: Some(Some(true)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (after, _) = svc.database_settings().await.unwrap();
    assert!(after.capture_enabled && after.capture_content == CaptureContent::Full);
}
