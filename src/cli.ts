#!/usr/bin/env bun
/**
 * rHarness — CLI (`rh`).
 *
 * Commands:
 *   rh serve [--port 4321] [--host 127.0.0.1]   Start the local web app
 *   rh run <task>                                Run the finish-first loop
 *   rh plugins                                   List built-in plugins
 *   rh config                                    Print the effective config
 *   rh init                                      Write a sample rharness.json
 *   rh --help                                    Show help
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { createServer } from "./web/server.js";
import { createHarness, registerBuiltinPlugins, defaultConfig } from "./core/index.js";

const argv = process.argv.slice(2);
const cmd = argv[0] ?? "serve";

function help(): void {
  console.log(`
rHarness — Finish First.

Usage:
  rh serve [--port 4321] [--host 127.0.0.1]
  rh run <task>
  rh plugins
  rh config
  rh init
  rh --help

Environment:
  RHA_PORT, RHA_HOST, RHA_API_KEY, RHA_BASE_URL, RHA_MODEL, RHA_MEMORY_FILE
`);
}

async function main(): Promise<void> {
  if (cmd === "--help" || cmd === "-h" || cmd === "help") {
    help();
    return;
  }

  if (cmd === "serve") {
    const portArg = argv.find((a) => a.startsWith("--port="));
    const hostArg = argv.find((a) => a.startsWith("--host="));
    const port = portArg ? Number(portArg.split("=")[1]) : Number(process.env.RHA_PORT ?? 4321);
    const host = hostArg?.split("=")[1] ?? process.env.RHA_HOST ?? "127.0.0.1";
    createServer({ port, host });
    return;
  }

  if (cmd === "run") {
    const task = argv.slice(1).join(" ").trim();
    if (!task) {
      console.error("Usage: rh run <task>");
      process.exit(1);
    }
    const harness = createHarness({ config: defaultConfig() });
    registerBuiltinPlugins(harness.registry);
    const run = await harness.runTask(task);
    console.log(JSON.stringify(run, null, 2));
    return;
  }

  if (cmd === "plugins") {
    const harness = createHarness({ config: defaultConfig() });
    registerBuiltinPlugins(harness.registry);
    for (const p of harness.registry.info()) {
      console.log(`  ${p.enabled ? "✓" : "○"}  ${p.id.padEnd(14)} ${p.name.padEnd(16)} ${p.version}`);
      console.log(`     ${p.description}`);
      console.log(`     phases: ${p.capabilities.phases.join(", ")}`);
      console.log(`     tools : ${p.capabilities.tools.join(", ")}`);
    }
    return;
  }

  if (cmd === "config") {
    const cfg = defaultConfig();
    // Mask secrets
    cfg.provider = { ...cfg.provider };
    console.log(JSON.stringify(cfg, null, 2));
    return;
  }

  if (cmd === "init") {
    const target = path.join(process.cwd(), "rharness.json");
    if (fs.existsSync(target)) {
      console.log(`${target} already exists — leaving it alone.`);
      return;
    }
    const sample = {
      name: "rHarness",
      version: "1.0.0",
      tagline: "Finish First.",
      loop: "finish-first",
      engine: "harness-core",
      license: "MIT",
      phases: ["Interrogate", "Contract", "Execute", "Finish"],
      plugins: ["web-search", "filesystem", "shell", "memory", "code-executor"],
      provider: {
        kind: "openai-compatible",
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o-mini",
        temperature: 0.2,
        max_tokens: 1024,
      },
    };
    fs.writeFileSync(target, JSON.stringify(sample, null, 2) + "\n");
    console.log(`Wrote ${target}`);
    return;
  }

  console.error(`Unknown command: ${cmd}\n`);
  help();
  process.exit(1);
}

main().catch((err) => {
  console.error(err instanceof Error ? err.stack ?? err.message : String(err));
  process.exit(1);
});
