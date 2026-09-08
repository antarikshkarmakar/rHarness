/**
 * Mythos Harness — entry point.
 *
 *   bun src/index.ts          → start the local web app
 *   bun src/index.ts --ui     → same, opens the dashboard URL
 *
 * The CLI (src/cli.ts) provides richer subcommands (`run`, `plugins`,
 * `init`, `serve`) and is wired to the `rh` bin in package.json.
 */

import { spawn } from "node:child_process";
import { createServer } from "./web/server.js";

const args = process.argv.slice(2);
const openUi = args.includes("--ui") || args.includes("-u");

const port = Number(process.env.RHA_PORT ?? 4321);
const host = process.env.RHA_HOST ?? "127.0.0.1";

const app = createServer({ port, host });

if (openUi) {
  const url = `http://${host}:${port}/dashboard`;
  const cmd = process.platform === "darwin" ? "open" : process.platform === "win32" ? "start" : "xdg-open";
  try {
    const child = spawn(cmd, [url], { stdio: "ignore", detached: true });
    child.on("error", () => {});
    child.unref?.();
  } catch {
    // Ignore — user can open the URL manually.
  }
}

process.on("SIGINT", () => {
  console.log("\n  Shutting down…");
  app.stop();
  process.exit(0);
});
process.on("SIGTERM", () => {
  app.stop();
  process.exit(0);
});
