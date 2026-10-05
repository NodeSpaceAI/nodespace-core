#!/usr/bin/env bun
/**
 * Download ML models for development or bundling with the application
 *
 * Usage:
 *   bun run download:models          # Download to ~/.nodespace/models/ (development)
 *   bun run download:models --bundle # Download to resources/ (CI/CD build)
 */

import { $ } from "bun";
import { existsSync, mkdirSync, rmSync } from "fs";
import { join } from "path";
import { homedir } from "os";

const MODEL_FILE = "nomic-embed-text-v1.5.Q8_0.gguf";

// Model artifact integrity (ADR-058, Threat T9 supply chain). This build-time
// download bakes the model into the signed, notarized bundle, so a substituted
// artifact would ship as authentic. The pinned SHA-256 below is the
// authoritative integrity gate; the pinned HuggingFace commit stops `main` from
// moving under us. Rotating the model MUST update BOTH the commit and the digest
// here AND the matching `EMBEDDING_MODEL_SHA256` in
// `packages/nlp-engine/src/config.rs` and in
// `packages/desktop-app/app-lib/src/bundled_model.rs` (the app's copy of its
// bundled model).
const MODEL_HF_COMMIT = "0188c9bf409793f810680a5a431e7b899c46104c";
const MODEL_SHA256 =
  "3e24342164b3d94991ba9692fdc0dd08e3fd7362e0aacc396a9a5c54a544c3b7";
const MODEL_URL = `https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/${MODEL_HF_COMMIT}/${MODEL_FILE}`;

/**
 * Compute the SHA-256 of a file, streaming it through the hasher so a ~146 MB
 * GGUF doesn't get buffered whole in memory. Returns the lowercase-hex digest.
 */
async function sha256File(path: string): Promise<string> {
  const hasher = new Bun.CryptoHasher("sha256");
  const stream = Bun.file(path).stream();
  for await (const chunk of stream) {
    hasher.update(chunk);
  }
  return hasher.digest("hex");
}

// Determine target directory based on --bundle flag
const isBundleMode = process.argv.includes("--bundle");
const MODELS_DIR = isBundleMode
  ? "packages/desktop-app/src-tauri/resources/models"
  : join(homedir(), ".nodespace", "models");
const MODEL_PATH = join(MODELS_DIR, MODEL_FILE);

async function downloadModels() {
  const modeLabel = isBundleMode ? "bundling" : "development";
  console.log(`📦 Downloading embedding models for ${modeLabel}...`);
  console.log(`📁 Target directory: ${MODELS_DIR}`);

  mkdirSync(MODELS_DIR, { recursive: true });

  if (existsSync(MODEL_PATH)) {
    console.log("🔍 Verifying existing model...");
    const existing = await sha256File(MODEL_PATH);
    if (existing === MODEL_SHA256) {
      console.log("✅ Model already downloaded and verified");
      return;
    }
    console.warn(
      `⚠️  Existing model failed integrity check (expected ${MODEL_SHA256}, got ${existing}); re-downloading.`,
    );
    rmSync(MODEL_PATH, { force: true });
  }

  console.log(`⬇️  Downloading ${MODEL_FILE}...`);
  // HTTPS with normal cert validation and hard failure on HTTP errors. The
  // pinned digest verified below is the authoritative gate regardless of where
  // any redirect ultimately points.
  await $`curl -fL --proto =https --progress-bar -o ${MODEL_PATH} ${MODEL_URL}`;

  console.log("🔍 Verifying SHA-256...");
  const digest = await sha256File(MODEL_PATH);
  if (digest !== MODEL_SHA256) {
    rmSync(MODEL_PATH, { force: true });
    throw new Error(
      `❌ Model integrity check FAILED: expected SHA-256 ${MODEL_SHA256}, got ${digest}. ` +
        `Deleted the downloaded file — refusing to bundle an unverified model.`,
    );
  }

  console.log(`✅ Model downloaded and verified: ${MODEL_PATH}`);

  const size = await $`du -sh ${MODEL_PATH}`.text();
  console.log(`📊 Model size: ${size.trim()}`);
}

// Run if called directly
if (import.meta.main) {
  await downloadModels();
}
