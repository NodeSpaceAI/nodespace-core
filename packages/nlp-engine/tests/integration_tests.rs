/// Integration tests for the embedding service
/// These tests require the nomic-embed-vision GGUF model to be downloaded
/// See docs/architecture/components/nlp-model-setup.md for download instructions
#[cfg(all(test, feature = "embedding-service"))]
mod integration_tests {
    use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService, EMBEDDING_DIMENSION};

    fn model_exists() -> bool {
        let config = EmbeddingConfig::default();
        config.resolve_model_path().is_ok()
    }

    #[tokio::test]
    async fn test_service_initialization() {
        if !model_exists() {
            eprintln!(
                "Skipping test: model not found. \
                See docs/architecture/components/nlp-model-setup.md for setup instructions."
            );
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();

        let result = service.initialize();
        assert!(result.is_ok(), "Failed to initialize service: {:?}", result);
        assert!(service.is_initialized());
    }

    #[tokio::test]
    async fn test_single_embedding_generation() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let text = "This is a test sentence for embedding generation.";
        let embedding = service.generate_embedding(text);

        assert!(embedding.is_ok(), "Failed to generate embedding");
        let embedding = embedding.unwrap();

        // nomic-embed-vision-v1.5 produces 768-dimensional embeddings
        assert_eq!(
            embedding.len(),
            EMBEDDING_DIMENSION,
            "Expected {} dimensions, got {}",
            EMBEDDING_DIMENSION,
            embedding.len()
        );

        // Check that embeddings are not all zeros
        let non_zero_count = embedding.iter().filter(|&&x| x != 0.0).count();
        assert!(non_zero_count > 0, "Embedding should not be all zeros");
    }

    #[tokio::test]
    async fn test_asymmetric_embeddings() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let text = "Machine learning for natural language processing";

        // Document and query embeddings should be different due to prefixes
        let doc_embedding = service.embed_document(text).unwrap();
        let query_embedding = service.embed_query(text).unwrap();

        assert_eq!(doc_embedding.len(), EMBEDDING_DIMENSION);
        assert_eq!(query_embedding.len(), EMBEDDING_DIMENSION);

        // They should be different (asymmetric prefixes)
        let diff: f32 = doc_embedding
            .iter()
            .zip(query_embedding.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(diff > 0.01, "Document and query embeddings should differ");
    }

    #[tokio::test]
    async fn test_batch_embedding_generation() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let texts = vec![
            "First test sentence",
            "Second test sentence",
            "Third test sentence",
        ];

        let embeddings = service.generate_batch(texts.clone());
        assert!(embeddings.is_ok(), "Failed to generate batch embeddings");

        let embeddings = embeddings.unwrap();
        assert_eq!(embeddings.len(), texts.len());

        for embedding in embeddings {
            assert_eq!(embedding.len(), EMBEDDING_DIMENSION);
        }
    }

    #[tokio::test]
    async fn test_cache_functionality() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let text = "Cache test sentence";

        // Generate embedding first time
        let embedding1 = service.generate_embedding(text).unwrap();

        // Check cache stats
        let (cache_size, _capacity) = service.cache_stats();
        assert_eq!(cache_size, 1, "Cache should have 1 entry");

        // Generate same embedding again (should hit cache)
        let embedding2 = service.generate_embedding(text).unwrap();

        // Embeddings should be identical
        assert_eq!(embedding1.len(), embedding2.len());
        for (e1, e2) in embedding1.iter().zip(embedding2.iter()) {
            assert!(
                (e1 - e2).abs() < 1e-6,
                "Cached embedding differs from original"
            );
        }

        // Clear cache
        service.clear_cache();
        let (cache_size, _) = service.cache_stats();
        assert_eq!(cache_size, 0, "Cache should be empty after clear");
    }

    #[tokio::test]
    async fn test_semantic_similarity() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let text1 = "The cat sits on the mat";
        let text2 = "A feline rests on a rug";
        let text3 = "Python is a programming language";

        let emb1 = service.embed_document(text1).unwrap();
        let emb2 = service.embed_document(text2).unwrap();
        let emb3 = service.embed_document(text3).unwrap();

        // Cosine similarity helper
        fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
            let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
            let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
            dot_product / (norm_a * norm_b)
        }

        let sim_1_2 = cosine_similarity(&emb1, &emb2);
        let sim_1_3 = cosine_similarity(&emb1, &emb3);

        // Semantically similar sentences should have higher similarity
        assert!(
            sim_1_2 > sim_1_3,
            "Similar sentences should have higher cosine similarity. sim(1,2)={}, sim(1,3)={}",
            sim_1_2,
            sim_1_3
        );
    }

    #[tokio::test]
    async fn test_blob_roundtrip() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let text = "Test for blob conversion";
        let embedding = service.generate_embedding(text).unwrap();

        // Convert to blob
        let blob = EmbeddingService::to_blob(&embedding);

        // Verify blob size (4 bytes per f32 * 768 dimensions)
        assert_eq!(blob.len(), EMBEDDING_DIMENSION * 4);

        // Convert back
        let recovered = EmbeddingService::from_blob(&blob);

        // Verify recovery
        assert_eq!(embedding.len(), recovered.len());
        for (original, recovered) in embedding.iter().zip(recovered.iter()) {
            assert!(
                (original - recovered).abs() < 1e-6,
                "Blob conversion lost precision"
            );
        }
    }

    #[tokio::test]
    async fn test_empty_text_handling() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        // Empty string should return an error
        let result = service.generate_embedding("");
        assert!(result.is_err(), "Empty string should return error");
    }

    #[tokio::test]
    async fn test_long_text_handling() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        // Text longer than typical context (will be truncated by tokenizer)
        let long_text = "word ".repeat(1000);

        let result = service.generate_embedding(&long_text);
        assert!(result.is_ok(), "Should handle long texts (with truncation)");

        let embedding = result.unwrap();
        assert_eq!(embedding.len(), EMBEDDING_DIMENSION);
    }

    #[tokio::test]
    async fn test_device_info() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let device_info = service.device_info();

        // Should report llama.cpp with GPU layers
        assert!(
            device_info.contains("llama.cpp"),
            "Device info should report llama.cpp, got: {}",
            device_info
        );
    }

    #[tokio::test]
    async fn test_concurrent_requests() {
        if !model_exists() {
            eprintln!("Skipping test: model not found");
            return;
        }

        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let service = std::sync::Arc::new(tokio::sync::Mutex::new(service));

        // Spawn multiple concurrent embedding requests
        let mut handles = vec![];
        for i in 0..10 {
            let service_clone = service.clone();
            let handle = tokio::spawn(async move {
                let service = service_clone.lock().await;
                let text = format!("Concurrent request {}", i);
                service.generate_embedding(&text)
            });
            handles.push(handle);
        }

        // Wait for all requests to complete
        let results: Vec<_> = futures::future::join_all(handles).await;

        // All requests should succeed
        for result in results {
            assert!(result.is_ok(), "Concurrent request failed");
            let embedding = result.unwrap();
            assert!(embedding.is_ok(), "Embedding generation failed");
            assert_eq!(embedding.unwrap().len(), EMBEDDING_DIMENSION);
        }
    }
}

/// Stub tests that run even without the model
#[cfg(all(test, not(feature = "embedding-service")))]
mod stub_tests {
    use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService, EMBEDDING_DIMENSION};

    #[tokio::test]
    async fn test_stub_service_creation() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config);
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_stub_initialization() {
        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        let result = service.initialize();
        assert!(result.is_ok());
        assert!(service.is_initialized());
    }

    #[tokio::test]
    async fn test_stub_embedding() {
        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let embedding = service.generate_embedding("test").unwrap();
        assert_eq!(embedding.len(), EMBEDDING_DIMENSION);
        assert!(embedding.iter().all(|&x| x == 0.0));
    }
}
