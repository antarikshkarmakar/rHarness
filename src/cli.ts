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
 *   rh local list|status|start|stop|test         Manage local model servers
 *   rh --help                                    Show help
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { createServer } from "./web/server.js";
import { createHarness, registerBuiltinPlugins, defaultConfig } from "./core/index.js";
import { createLocalModelManager } from "./core/localmodel.js";

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
  rh local list
  rh local status <profile-id>
  rh local start <profile-id> [--no-launch]
  rh local stop <profile-id>
  rh local test <profile-id>
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

  if (cmd === "local") {
    const sub = argv[1];
    const mgr = createLocalModelManager();
    if (!sub || sub === "list") {
      console.log("Local model profiles:\n");
      for (const p of mgr.list()) {
        console.log(`  ${p.id.padEnd(22)} ${p.name}`);
        console.log(`     format : ${p.format}`);
        console.log(`     base   : ${p.base_url}`);
        console.log(`     model  : ${p.model}`);
        if (p.start) console.log(`     start  : ${p.start}`);
        if (p.stop) console.log(`     stop   : ${p.stop}`);
        if (p.notes) console.log(`     ${p.notes}`);
        console.log("");
      }
      console.log("  rh local status|start|stop|test <id>");
      return;
    }

    const id = argv[2];
    if (!id) {
      console.error(`Usage: rh local ${sub} <profile-id>`);
      console.error("Run `rh local list` to see available profiles.");
      process.exit(1);
    }

    if (sub === "status") {
      const s = await mgr.status(id);
      if (s.running) {
        console.log(`✓ ${s.base_url}  (${s.latency_ms ?? "?"} ms)`);
        if (s.models?.length) console.log(`  models: ${s.models.join(", ")}`);
      } else {
        console.log(`✗ ${s.base_url}  — ${s.error ?? "not running"}`);
        process.exit(1);
      }
      return;
    }

    if (sub === "start") {
      const noLaunch = argv.includes("--no-launch");
      console.log(`Starting local server for "${id}"…`);
      const r = await mgr.start(id, { launch: !noLaunch });
      if (r.ok) {
        console.log(`✓ Server is up: ${r.status.base_url}`);
        if (r.status.models?.length) console.log(`  models: ${r.status.models.join(", ")}`);
      } else {
        console.log(`✗ ${r.error ?? "failed to start"}`);
        if (r.launched?.stderr) console.log(r.launched.stderr.slice(0, 500));
        process.exit(1);
      }
      return;
    }

    if (sub === "stop") {
      const r = await mgr.stop(id);
      if (r.ok) console.log(`✓ Stopped "${id}"`);
      else {
        console.log(`✗ ${r.error ?? "failed to stop"}`);
        process.exit(1);
      }
      return;
    }

    if (sub === "test") {
      console.log(`Sending test completion to "${id}"…`);
      const r = await mgr.test(id);
      if (r.ok) {
        console.log(`✓ ${r.latency_ms} ms — model: ${r.model}`);
        console.log(`  reply: ${r.reply?.slice(0, 200)}`);
      } else {
        console.log(`✗ ${r.error ?? "test failed"}`);
        process.exit(1);
      }
      return;
    }

    console.error(`Unknown subcommand: ${sub}`);
    console.error("Available: list, status, start, stop, test");
    process.exit(1);
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
