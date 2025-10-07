#!/usr/bin/env bun
/**
 * Download ML models for bundling with the application
 * This script is run during CI/CD build process, not by end users
 */

import { $ } from "bun";
import { existsSync, mkdirSync } from "fs";
import { join } from "path";

const MODELS_DIR = "packages/desktop-app/src-tauri/resources/models";
const MODEL_NAME = "BAAI-bge-small-en-v1.5";
const MODEL_PATH = join(MODELS_DIR, MODEL_NAME);

async function downloadModels() {
  console.log("📦 Downloading embedding models for bundling...");

  // Create models directory
  mkdirSync(MODELS_DIR, { recursive: true });

  // Check if model already exists
  if (existsSync(MODEL_PATH)) {
    console.log("✅ Model already downloaded");
    return;
  }

  // Check if huggingface-cli is available
  try {
    await $`which huggingface-cli`.quiet();
  } catch {
    console.log("❌ huggingface-cli not found. Installing...");
    await $`pip install huggingface-hub`;
  }

  // Download model
  console.log(`⬇️  Downloading BAAI/bge-small-en-v1.5...`);
  await $`huggingface-cli download BAAI/bge-small-en-v1.5 \
    --local-dir ${MODEL_PATH} \
    --exclude pytorch_model.bin tf_model.h5 \
    --quiet`;

  console.log(`✅ Model downloaded to ${MODEL_PATH}`);

  // Show model size
  const size = await $`du -sh ${MODEL_PATH}`.text();
  console.log(`📊 Model size: ${size.trim()}`);
}

// Run if called directly
if (import.meta.main) {
  await downloadModels();
}
