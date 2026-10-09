#!/usr/bin/env bun
/**
 * `eval:grounding` entry point — see ./fixtures/grounding.ts for the scenarios
 * and scoring, and ./runner.ts for everything else.
 */

import fixture from "./fixtures/grounding.ts";
import { runEval } from "./runner.ts";

await runEval(fixture);
