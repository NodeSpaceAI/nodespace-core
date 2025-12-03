use nodespace_core::db::SurrealStore;
use nodespace_core::models::Node;
use serde_json::json;

#[tokio::test]
async fn test_schema_property_hydration() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary database
    let temp_dir = std::env::temp_dir().join(format!("test_schema_{}", uuid::Uuid::new_v4()));
    let store = SurrealStore::new(temp_dir.clone()).await?;

    println!("✅ Database initialized");

    // Debug: Check if core schemas were seeded by trying to get one
    println!("📊 Checking if core schemas are present...");
    match store.get_node("task").await {
        Ok(Some(node)) => println!("✅ Found task schema: {}", node.id),
        Ok(None) => println!("❌ Task schema not found"),
        Err(e) => println!("❌ Error checking for task schema: {}", e),
    }

    // Create a simple schema node with minimal properties
    let schema_node = Node::new_with_id(
        "simple".to_string(),
        "schema".to_string(),
        "Simple Schema".to_string(),
        json!({"test": "value"}),
    );

    println!("📝 Creating schema node with ID 'date'...");
    let created = store.create_node(schema_node, None).await?;
    println!("✅ Schema node created: {:?}", created.id);
    println!(
        "    Properties on created node: {}",
        serde_json::to_string_pretty(&created.properties)?
    );

    // Retrieve the schema node via get_node
    println!("\n🔍 Retrieving schema node via get_node()...");

    // Debug: Check what's in the node table using get_node
    println!("📊 Checking node table...");
    match store.get_node("simple").await {
        Ok(Some(node)) => println!("✅ Found node: {} (type: {})", node.id, node.node_type),
        Ok(None) => println!("❌ Node not found"),
        Err(e) => println!("❌ Error checking node: {}", e),
    }

    // Debug: Check what's in the schema table
    println!("📊 Checking schema table...");
    let schema_query = "SELECT * FROM schema;";
    let mut schema_response = store.db().query(schema_query).await;
    match schema_response {
        Ok(ref mut response) => {
            let schema_records: Result<Vec<serde_json::Value>, _> = response.take(0);
            match schema_records {
                Ok(records) => println!("Records in schema table: {:?}", records),
                Err(e) => println!("Error deserializing schema table: {:?}", e),
            }
        }
        Err(e) => println!("Error querying schema table: {:?}", e),
    }

    let retrieved = store.get_node("simple").await?;

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
    let schema = store.get_schema("simple").await?;

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
