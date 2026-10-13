#!/usr/bin/env bun
/**
 * `eval:tasks` entry point — see ./fixtures/tasks.ts for the scenarios and
 * scoring, and ./runner.ts for everything else.
 */

import fixture from "./fixtures/tasks.ts";
import { runEval } from "./runner.ts";

await runEval(fixture);
