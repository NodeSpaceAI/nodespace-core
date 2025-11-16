use nodespace_core::db::SurrealStore;
use nodespace_core::models::Node;
use serde_json::json;

#[tokio::test]
async fn test_schema_property_hydration() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary database
    let temp_dir = std::env::temp_dir().join(format!("test_schema_{}", uuid::Uuid::new_v4()));
    let store = SurrealStore::new(temp_dir.clone()).await?;

    println!("✅ Database initialized");

    // Create a schema node with properties
    let schema_node = Node::new_with_id(
        "date".to_string(),
        "schema".to_string(),
        "Date".to_string(),
        None,
        json!({
            "is_core": true,
            "version": 1,
            "description": "Date node schema",
            "fields": []
        }),
    );

    println!("📝 Creating schema node with ID 'date'...");
    let created = store.create_node(schema_node).await?;
    println!("✅ Schema node created: {:?}", created.id);
    println!(
        "    Properties on created node: {}",
        serde_json::to_string_pretty(&created.properties)?
    );

    // Retrieve the schema node via get_node
    println!("\n🔍 Retrieving schema node via get_node()...");
    let retrieved = store.get_node("date").await?;

    match retrieved {
        Some(node) => {
            println!("✅ Schema node retrieved!");
            println!("   ID: {}", node.id);
            println!("   Type: {}", node.node_type);
            println!(
                "   Properties: {}",
                serde_json::to_string_pretty(&node.properties)?
            );

            // Verify properties are not empty
            assert!(
                !node.properties.as_object().unwrap().is_empty(),
                "Properties should not be empty!"
            );

            println!("✅ Properties hydrated correctly!");
        }
        None => {
            panic!("❌ Schema node not found after creation!");
        }
    }

    // Test via get_schema method
    println!("\n🔍 Testing get_schema method...");
    let schema = store.get_schema("date").await?;

    match schema {
        Some(props) => {
            println!("✅ get_schema returned properties:");
            println!("{}", serde_json::to_string_pretty(&props)?);
        }
        None => {
            panic!("❌ get_schema returned None!");
        }
    }

    // Cleanup
    std::fs::remove_dir_all(&temp_dir)?;

    Ok(())
}
