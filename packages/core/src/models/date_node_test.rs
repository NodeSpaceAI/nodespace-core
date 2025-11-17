//! Tests for DateNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{DateNode, Node};
    use chrono::NaiveDate;
    use serde_json::json;

    #[test]
    fn test_from_node_validates_type() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        assert!(DateNode::from_node(node).is_ok());

        let wrong_type = Node::new_with_id(
            "2025-01-15".to_string(),
            "text".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let result = DateNode::from_node(wrong_type);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'date'"));
    }

    #[test]
    fn test_from_node_validates_id_format() {
        let invalid_id = Node::new_with_id(
            "invalid-id".to_string(),
            "date".to_string(),
            "invalid-id".to_string(),
            json!({}),
        );
        let result = DateNode::from_node(invalid_id);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("YYYY-MM-DD"));
    }

    #[test]
    fn test_date_getter() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let date_node = DateNode::from_node(node).unwrap();

        let date = date_node.date().unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
    }

    #[test]
    fn test_timezone_getter() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({"timezone": "America/New_York"}),
        );
        let date_node = DateNode::from_node(node).unwrap();

        assert_eq!(date_node.timezone(), Some("America/New_York".to_string()));
    }

    #[test]
    fn test_timezone_getter_none() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let date_node = DateNode::from_node(node).unwrap();

        assert_eq!(date_node.timezone(), None);
    }

    #[test]
    fn test_timezone_setter() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let mut date_node = DateNode::from_node(node).unwrap();

        date_node.set_timezone(Some("Europe/London".to_string()));

        assert_eq!(date_node.timezone(), Some("Europe/London".to_string()));
        assert_eq!(date_node.as_node().properties["timezone"], "Europe/London");
    }

    #[test]
    fn test_timezone_clear() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({"timezone": "UTC"}),
        );
        let mut date_node = DateNode::from_node(node).unwrap();

        date_node.set_timezone(None);

        assert_eq!(date_node.timezone(), None);
        assert!(date_node.as_node().properties.get("timezone").is_none());
    }

    #[test]
    fn test_is_holiday_getter() {
        let node = Node::new_with_id(
            "2025-12-25".to_string(),
            "date".to_string(),
            "2025-12-25".to_string(),
            json!({"is_holiday": true}),
        );
        let date_node = DateNode::from_node(node).unwrap();

        assert!(date_node.is_holiday());
    }

    #[test]
    fn test_is_holiday_getter_default() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let date_node = DateNode::from_node(node).unwrap();

        assert!(!date_node.is_holiday());
    }

    #[test]
    fn test_is_holiday_setter() {
        let node = Node::new_with_id(
            "2025-07-04".to_string(),
            "date".to_string(),
            "2025-07-04".to_string(),
            json!({}),
        );
        let mut date_node = DateNode::from_node(node).unwrap();

        date_node.set_is_holiday(true);

        assert!(date_node.is_holiday());
        assert_eq!(date_node.as_node().properties["is_holiday"], true);
    }

    #[test]
    fn test_into_node_preserves_data() {
        let original = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({"timezone": "UTC", "is_holiday": false}),
        );

        let date_node = DateNode::from_node(original).unwrap();
        let converted_back = date_node.into_node();

        assert_eq!(converted_back.id, "2025-01-15");
        assert_eq!(converted_back.node_type, "date");
        assert_eq!(converted_back.content, "2025-01-15");
        assert_eq!(converted_back.properties["timezone"], "UTC");
        assert_eq!(converted_back.properties["is_holiday"], false);
    }

    #[test]
    fn test_as_node_reference() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let date_node = DateNode::from_node(node).unwrap();

        let node_ref = date_node.as_node();
        assert_eq!(node_ref.node_type, "date");
        assert_eq!(node_ref.id, "2025-01-15");
    }

    #[test]
    fn test_as_node_mut() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let mut date_node = DateNode::from_node(node).unwrap();

        date_node.as_node_mut().content = "2025-01-15 (Modified)".to_string();

        assert_eq!(date_node.as_node().content, "2025-01-15 (Modified)");
    }

    #[test]
    fn test_builder_minimal() {
        let date = NaiveDate::from_ymd_opt(2025, 3, 20).unwrap();
        let date_node = DateNode::for_date(date).build();

        assert_eq!(date_node.as_node().id, "2025-03-20");
        assert_eq!(date_node.as_node().node_type, "date");
        assert_eq!(date_node.as_node().content, "2025-03-20");
        assert_eq!(date_node.date().unwrap(), date);
    }

    #[test]
    fn test_builder_with_timezone() {
        let date = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let date_node = DateNode::for_date(date)
            .with_timezone("Asia/Tokyo".to_string())
            .build();

        assert_eq!(date_node.timezone(), Some("Asia/Tokyo".to_string()));
    }

    #[test]
    fn test_builder_with_is_holiday() {
        let date = NaiveDate::from_ymd_opt(2025, 12, 25).unwrap();
        let date_node = DateNode::for_date(date).with_is_holiday(true).build();

        assert!(date_node.is_holiday());
    }

    #[test]
    fn test_builder_full() {
        let date = NaiveDate::from_ymd_opt(2025, 7, 4).unwrap();
        let date_node = DateNode::for_date(date)
            .with_timezone("America/New_York".to_string())
            .with_is_holiday(true)
            .build();

        assert_eq!(date_node.date().unwrap(), date);
        assert_eq!(date_node.timezone(), Some("America/New_York".to_string()));
        assert!(date_node.is_holiday());
    }

    #[test]
    fn test_date_node_is_root() {
        let _date_node = DateNode::for_date(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()).build();

        // Date nodes should be roots (no parent relationship in edges)
        // Verified via graph structure, not fields
    }

    #[test]
    fn test_multiple_date_nodes() {
        let date1 = DateNode::for_date(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()).build();
        let date2 = DateNode::for_date(NaiveDate::from_ymd_opt(2025, 1, 2).unwrap()).build();

        assert_eq!(date1.as_node().id, "2025-01-01");
        assert_eq!(date2.as_node().id, "2025-01-02");
        assert_ne!(date1.as_node().id, date2.as_node().id);
    }

    #[test]
    fn test_date_id_deterministic() {
        let date = NaiveDate::from_ymd_opt(2025, 5, 15).unwrap();
        let date_node1 = DateNode::for_date(date).build();
        let date_node2 = DateNode::for_date(date).build();

        // Both should have the same ID (deterministic)
        assert_eq!(date_node1.as_node().id, date_node2.as_node().id);
        assert_eq!(date_node1.as_node().id, "2025-05-15");
    }

    #[test]
    fn test_various_date_formats() {
        let dates = vec![
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(), // Start of year
            NaiveDate::from_ymd_opt(2025, 12, 31).unwrap(), // End of year
            NaiveDate::from_ymd_opt(2025, 2, 28).unwrap(), // Regular date
            NaiveDate::from_ymd_opt(2024, 2, 29).unwrap(), // Leap year
        ];

        for date in dates {
            let date_node = DateNode::for_date(date).build();
            assert_eq!(date_node.date().unwrap(), date);
        }
    }

    #[test]
    fn test_invalid_date_id_format_variations() {
        let invalid_ids = vec![
            "2025/01/15", // Wrong separator
            "20250115",   // No separators
            "not-a-date", // Completely invalid
            "2025-13-01", // Invalid month
            "2025-01-32", // Invalid day
            "2025-02-30", // Invalid date for February
        ];

        for invalid_id in invalid_ids {
            let node = Node::new_with_id(
                invalid_id.to_string(),
                "date".to_string(),
                invalid_id.to_string(),
                json!({}),
            );
            let result = DateNode::from_node(node);
            assert!(result.is_err(), "Expected error for ID: {}", invalid_id);
        }
    }

    #[test]
    fn test_properties_modification() {
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let mut date_node = DateNode::from_node(node).unwrap();

        // Modify multiple properties
        date_node.set_timezone(Some("Pacific/Auckland".to_string()));
        date_node.set_is_holiday(true);

        // Verify all modifications
        assert_eq!(date_node.timezone(), Some("Pacific/Auckland".to_string()));
        assert!(date_node.is_holiday());
    }
}
