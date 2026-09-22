#!/usr/bin/env bun
/**
 * `eval:decisions` entry point — see ./fixtures/decisions.ts for the scenarios
 * and scoring, and ./runner.ts for everything else.
 */

import fixture from "./fixtures/decisions.ts";
import { runEval } from "./runner.ts";

await runEval(fixture);
