#!/usr/bin/env bun
/**
 * Download ML models for development or bundling with the application
 *
 * Usage:
 *   bun run download:models          # Download to ~/.nodespace/models/ (development)
 *   bun run download:models --bundle # Download to resources/ (CI/CD build)
 */

import { $ } from "bun";
import { existsSync, mkdirSync } from "fs";
import { join } from "path";
import { homedir } from "os";

const MODEL_NAME = "BAAI-bge-small-en-v1.5";

// Determine target directory based on --bundle flag
const isBundleMode = process.argv.includes("--bundle");
const MODELS_DIR = isBundleMode
  ? "packages/desktop-app/src-tauri/resources/models"
  : join(homedir(), ".nodespace", "models");
const MODEL_PATH = join(MODELS_DIR, MODEL_NAME);

async function downloadModels() {
  const modeLabel = isBundleMode ? "bundling" : "development";
  console.log(`📦 Downloading embedding models for ${modeLabel}...`);
  console.log(`📁 Target directory: ${MODELS_DIR}`);

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
