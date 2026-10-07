//! Two databases open in one daemon each behave by their own settings
//! (ADR-095): the providers the local agent resolves, the capture choice a
//! session starts with, and whether external tools are served.

use std::sync::Arc;

use nodespace_agent::pty::PtySessionManager;
use nodespace_daemon::nodespace::local_agent_service_server::LocalAgentService as _;
use nodespace_daemon::nodespace::LoadModelRequest;
use nodespace_daemon::services::capture_service::CaptureConfig;
use nodespace_daemon::{DatabaseManager, DatabaseServices, SharedContext};
use nodespace_nlp_engine::EmbeddingService;
use nodespace_types::{
    CaptureContent, DatabaseSettingsNodeUpdate, ProviderConfig, DATABASE_SETTINGS_NODE_ID,
};
use tempfile::TempDir;
use tokio::sync::watch;
use tonic::Request;

const WORK_PROVIDER: &str = "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d";
const HOME_PROVIDER: &str = "1c2d3e4f-5061-4b7c-9d8e-0f1a2b3c4d5e";

fn context() -> SharedContext {
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        scheduler: Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
        subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
        local_agent: nodespace_daemon::SharedLocalAgent::new(),
        extensions: nodespace_daemon::DaemonExtensions::none(),
    }
}

fn provider(id: &str, name: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        name: name.to_string(),
        base_url: "http://127.0.0.1:9/v1".to_string(),
        api_key: "key".to_string(),
        model: "m".to_string(),
        routing_ok: Default::default(),
    }
}

async fn open(manager: &DatabaseManager, name: &str) -> Arc<DatabaseServices> {
    let id = manager
        .list()
        .await
        .databases
        .iter()
        .find(|d| d.entry.name == name)
        .expect("database registered")
        .entry
        .id
        .clone();
    manager.get_or_open(&id).await.expect("open database")
}

async fn write(services: &DatabaseServices, update: DatabaseSettingsNodeUpdate) {
    let node_service = services.node_service_grpc.node_service();
    let (_, version) = node_service.database_settings().await.unwrap();
    node_service
        .update_database_settings_node(DATABASE_SETTINGS_NODE_ID, version, update)
        .await
        .expect("write settings");
}

async fn load(services: &DatabaseServices, provider_id: &str) -> Result<(), tonic::Status> {
    services
        .local_agent
        .load_model(Request::new(LoadModelRequest {
            model_id: format!("openai-compat:{provider_id}"),
        }))
        .await
        .map(|_| ())
}

#[tokio::test]
async fn each_open_database_behaves_by_its_own_settings() {
    let dir = TempDir::new().unwrap();
    let manager = Arc::new(
        DatabaseManager::load(dir.path().join("databases.toml"), context())
            .await
            .unwrap(),
    );
    manager
        .ensure_default_registered("Work".into(), dir.path().join("work.db"))
        .await
        .unwrap();
    manager
        .create("Home".into(), Some(dir.path().join("home.db")))
        .await
        .unwrap();
    let work = open(&manager, "Work").await;
    let home = open(&manager, "Home").await;

    // A new database starts with capture off, external tools off and no
    // providers, and nothing is copied from another database.
    for services in [&work, &home] {
        let (settings, _) = services
            .node_service_grpc
            .node_service()
            .database_settings()
            .await
            .unwrap();
        assert!(!settings.capture_enabled);
        assert!(settings.providers.is_empty());
    }

    write(
        &work,
        DatabaseSettingsNodeUpdate {
            capture_enabled: Some(Some(true)),
            capture_content: Some(Some(CaptureContent::Full)),
            providers: Some(Some(vec![provider(WORK_PROVIDER, "Work endpoint")])),
            ..Default::default()
        },
    )
    .await;
    write(
        &home,
        DatabaseSettingsNodeUpdate {
            capture_content: Some(Some(CaptureContent::Summary)),
            providers: Some(Some(vec![provider(HOME_PROVIDER, "Home endpoint")])),
            ..Default::default()
        },
    )
    .await;

    // Capture: each reads the choice of the database the session is in.
    let capture = |settings| CaptureConfig::from(&settings);
    let (work_settings, _) = work
        .node_service_grpc
        .node_service()
        .database_settings()
        .await
        .unwrap();
    let (home_settings, _) = home
        .node_service_grpc
        .node_service()
        .database_settings()
        .await
        .unwrap();
    assert_eq!(
        capture(work_settings.clone()),
        CaptureConfig {
            enabled: true,
            content: CaptureContent::Full
        }
    );
    assert_eq!(
        capture(home_settings.clone()),
        CaptureConfig {
            enabled: false,
            content: CaptureContent::Summary
        }
    );

    // Providers: a config resolves in its own database and is not found in
    // the other.
    load(&work, WORK_PROVIDER).await.expect("work's provider");
    load(&home, HOME_PROVIDER).await.expect("home's provider");
    let missing = load(&work, HOME_PROVIDER)
        .await
        .expect_err("home's provider is not work's");
    assert!(
        missing
            .message()
            .contains("No OpenAI-compatible provider config found"),
        "{}",
        missing.message()
    );
    let missing = load(&home, WORK_PROVIDER)
        .await
        .expect_err("work's provider is not home's");
    assert!(
        missing
            .message()
            .contains("No OpenAI-compatible provider config found"),
        "{}",
        missing.message()
    );
}
