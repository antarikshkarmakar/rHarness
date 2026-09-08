/**
 * Mythos Harness — local web server.
 *
 * Endpoints:
 *   GET  /                  → launch page (assets/index.html)
 *   GET  /dashboard         → dashboard (assets/dashboard.html)
 *   GET  /static/...        → static assets (assets/*)
 *   GET  /api/health        → { ok, name, version, provider, plugins }
 *   GET  /api/state         → { runs, plugins }
 *   POST /api/chat          → single round-trip chat (JSON)
 *   POST /api/run           → run the finish-first loop on a task
 *   GET  /api/plugins       → list plugins
 *   POST /api/plugins       → { id, enabled }  enable/disable a plugin
 *   POST /api/runs/:id/cancel → abort a running task (best-effort)
 *
 * The server is intentionally tiny: one Bun.serve call, no framework,
 * and the UI is plain HTML + CSS + JS served from ./assets.
 */

import * as path from "node:path";
import { fileURLToPath } from "node:url";
import * as fs from "node:fs";
import { createHarness, registerBuiltinPlugins, defaultConfig, type Harness } from "../core/index.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const ASSETS_DIR = path.resolve(__dirname, "..", "..", "assets");

export interface ServerOptions {
  port?: number;
  host?: string;
  workingDir?: string;
}

export function createServer(options: ServerOptions = {}) {
  const config = defaultConfig();
  if (options.workingDir) config.working_dir = options.workingDir;

  const harness: Harness = createHarness({ config });
  registerBuiltinPlugins(harness.registry);

  const controllers = new Map<string, AbortController>();

  function json(data: unknown, status = 200): Response {
    return new Response(JSON.stringify(data), {
      status,
      headers: { "content-type": "application/json", "access-control-allow-origin": "*" },
    });
  }

  function file(filePath: string): Response {
    const full = path.join(ASSETS_DIR, filePath);
    if (!full.startsWith(ASSETS_DIR)) return new Response("Forbidden", { status: 403 });
    if (!fs.existsSync(full) || !fs.statSync(full).isFile()) {
      return new Response(`Not found: ${filePath}`, { status: 404 });
    }
    const f = Bun.file(full);
    const type = filePath.endsWith(".html")
      ? "text/html; charset=utf-8"
      : filePath.endsWith(".css")
        ? "text/css; charset=utf-8"
        : filePath.endsWith(".js")
          ? "text/javascript; charset=utf-8"
          : filePath.endsWith(".json")
            ? "application/json"
            : filePath.endsWith(".svg")
              ? "image/svg+xml"
              : "application/octet-stream";
    return new Response(f, { headers: { "content-type": type } });
  }

  async function handleApi(req: Request): Promise<Response> {
    const url = new URL(req.url);

    if (url.pathname === "/api/health" && req.method === "GET") {
      return json({
        ok: true,
        name: harness.config.name,
        version: harness.config.version,
        tagline: harness.config.tagline,
        loop: harness.config.loop_strategy,
        provider: harness.config.provider.kind,
        model: harness.config.provider.model,
        plugins: harness.registry.info().length,
      });
    }

    if (url.pathname === "/api/state" && req.method === "GET") {
      return json(harness.state());
    }

    if (url.pathname === "/api/chat" && req.method === "POST") {
      const body = (await req.json().catch(() => ({}))) as {
        messages?: { role: string; content: string }[];
        working_dir?: string;
      };
      const messages =
        body.messages?.map((m) => ({
          role: (m.role as "user" | "assistant" | "system" | "tool") ?? "user",
          content: String(m.content ?? ""),
          timestamp: new Date().toISOString(),
        })) ?? [];

      const runId = crypto.randomUUID();
      const controller = new AbortController();
      controllers.set(runId, controller);
      try {
        let reply = "";
        for await (const ev of harness.chat(messages, { signal: controller.signal })) {
          if (ev.type === "chat.completed") reply = String((ev.data as { content?: string }).content ?? "");
          else if (ev.type === "chat.error") {
            return json({ ok: false, error: String((ev.data as { error?: string }).error), run_id: runId }, 500);
          }
        }
        return json({ ok: true, run_id: runId, reply });
      } finally {
        controllers.delete(runId);
      }
    }

    if (url.pathname === "/api/run" && req.method === "POST") {
      const body = (await req.json().catch(() => ({}))) as {
        task?: string;
        working_dir?: string;
        max_iterations?: number;
      };
      const task = String(body.task ?? "").trim();
      if (!task) return json({ ok: false, error: "task is required" }, 400);

      const runId = crypto.randomUUID();
      const controller = new AbortController();
      controllers.set(runId, controller);

      try {
        const run = await harness.runTask(task, {
          working_dir: body.working_dir,
          max_iterations: body.max_iterations,
          signal: controller.signal,
        });
        return json({ ok: true, run });
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        return json({ ok: false, error: msg, run_id: runId }, 500);
      } finally {
        controllers.delete(runId);
      }
    }

    if (url.pathname === "/api/plugins" && req.method === "GET") {
      return json({ plugins: harness.registry.info() });
    }

    if (url.pathname === "/api/plugins" && req.method === "POST") {
      const body = (await req.json().catch(() => ({}))) as { id?: string; enabled?: boolean };
      if (!body.id || typeof body.enabled !== "boolean") {
        return json({ ok: false, error: "body must be { id, enabled }" }, 400);
      }
      const ok = harness.registry.setEnabled(body.id, body.enabled);
      if (!ok) return json({ ok: false, error: `unknown plugin "${body.id}"` }, 404);
      return json({ ok: true, plugins: harness.registry.info() });
    }

    const cancelMatch = url.pathname.match(/^\/api\/runs\/([^/]+)\/cancel$/);
    if (cancelMatch && req.method === "POST") {
      const id = decodeURIComponent(cancelMatch[1]!);
      const ctrl = controllers.get(id);
      if (!ctrl) return json({ ok: false, error: "no such run" }, 404);
      ctrl.abort();
      return json({ ok: true });
    }

    if (url.pathname.startsWith("/static/")) {
      const rel = url.pathname.slice("/static/".length);
      return file(rel);
    }

    if (url.pathname === "/") {
      return file("index.html");
    }
    if (url.pathname === "/dashboard" || url.pathname === "/app") {
      return file("dashboard.html");
    }

    return json({ ok: false, error: `no handler for ${req.method} ${url.pathname}` }, 404);
  }

  const port = options.port ?? Number(process.env.RHA_PORT ?? 4321);
  const host = options.host ?? process.env.RHA_HOST ?? "127.0.0.1";

  const server = Bun.serve({
    port,
    hostname: host,
    async fetch(req: Request): Promise<Response> {
      const url = new URL(req.url);
      if (url.pathname.startsWith("/api/") || url.pathname.startsWith("/static/") || url.pathname === "/" || url.pathname === "/dashboard" || url.pathname === "/app") {
        return handleApi(req);
      }
      // Fallback: treat as static asset
      return file(url.pathname.replace(/^\/+/, ""));
    },
    error(err) {
      return new Response(JSON.stringify({ ok: false, error: String(err) }), {
        status: 500,
        headers: { "content-type": "application/json" },
      });
    },
  });

  const startedAt = new Date().toISOString();
  console.log("");
  console.log("  ┌──────────────────────────────────────────────────┐");
  console.log("  │  rHarness — Finish First.                        │");
  console.log("  │                                                  │");
  console.log(`  │  http://${host}:${port}/            (launch page)   │`);
  console.log(`  │  http://${host}:${port}/dashboard    (web app)     │`);
  console.log(`  │  provider: ${harness.config.provider.kind.padEnd(22)} │`);
  console.log(`  │  plugins : ${harness.registry.info().length} built-in                 │`);
  console.log("  └──────────────────────────────────────────────────┘");
  console.log("");

  return {
    server,
    harness,
    port,
    host,
    startedAt,
    stop: () => server.stop(true),
  };
}
