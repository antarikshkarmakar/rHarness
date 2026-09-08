/**
 * rHarness — local-model profiles & server manager.
 *
 * rHarness drives a **locally-served** model through its OpenAI-compatible
 * endpoint. This module makes that easy and format-agnostic:
 *
 *  - EXL3  (vLLM / tabbyAPI)      — the target today (GLM-5.3-Flash on DGX Spark)
 *  - GGUF  (llama.cpp / Ollama / LM Studio)
 *  - NVFP4 (vLLM)
 *
 * Each profile declares the endpoint (`base_url` + `model`) plus optional
 * `start` / `stop` shell commands. When you have a recipe (e.g. 0xSero's
 * `start-rank-stacked-tp1.sh`) you point the profile's `start`/`stop` at it and
 * rHarness can bring the server up, wait for `/v1` to be healthy, and chat —
 * all from one place. A profile with no `start` command is fine too: just
 * launch the server yourself and rHarness talks to it.
 */

import { spawn } from "node:child_process";

export type ModelFormat = "exl3" | "gguf" | "nvfp4" | "openai" | "other";

export interface LocalModelProfile {
  /** Stable id, e.g. `glm53-exl3-spark`. */
  id: string;
  /** Human name, e.g. `GLM-5.3-Flash EXL3 (single Spark)`. */
  name: string;
  /** Weight/quant format the served model uses. */
  format: ModelFormat;
  /** OpenAI-compatible base, e.g. `http://127.0.0.1:18080/v1`. */
  base_url: string;
  /** Served model id, e.g. `glm-5.3-flash-exl3-k2-single-spark`. */
  model: string;
  /** Working directory for `start`/`stop` (defaults to process.cwd()). */
  cwd?: string;
  /** Optional env for the launch command. */
  env?: Record<string, string>;
  /** Shell command that starts the local server. Optional. */
  start?: string;
  /** Shell command that stops the local server. Optional. */
  stop?: string;
  notes?: string;
}

/** Built-in profiles. Add GGUF / NVFP4 ones the same way. */
export const DEFAULT_PROFILES: LocalModelProfile[] = [
  {
    id: "glm53-exl3-spark",
    name: "GLM-5.3-Flash EXL3 2.0bpw (single DGX Spark)",
    format: "exl3",
    base_url: "http://127.0.0.1:18080/v1",
    model: "glm-5.3-flash-exl3-k2-single-spark",
    start: "./runtime/spark/start-rank-stacked-tp1.sh",
    stop: "./stop.sh",
    notes:
      "0xSero single-Spark recipe. Set `cwd` to the recipe checkout; ensure " +
      "weights are downloaded and the image pulled before `start`.",
  },
  {
    id: "glm53-exl3-2spark",
    name: "GLM-5.3-Flash EXL3 4bpw (2× DGX Spark)",
    format: "exl3",
    base_url: "http://127.0.0.1:8888/v1",
    model: "GLM-5.3-Flash-EXL3",
    start: "./start.sh",
    stop: "./stop.sh",
    notes: "MiaAI-Lab 2-Spark recipe over CX7. Set `cwd` to the recipe checkout.",
  },
  {
    id: "gguf-ollama",
    name: "GGUF via Ollama /v1",
    format: "gguf",
    base_url: "http://127.0.0.1:11434/v1",
    model: "llama3",
    notes: "Ollama exposes an OpenAI-compatible /v1. Replace `model` with your Ollama tag.",
  },
];

export interface LocalStatus {
  running: boolean;
  base_url: string;
  model: string;
  /** Model ids reported by the local server (if reachable). */
  models?: string[];
  /** Milliseconds to first healthy response, when running. */
  latency_ms?: number;
  error?: string;
}

async function probe(base_url: string, timeoutMs: number): Promise<LocalStatus> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  const started = Date.now();
  try {
    const res = await fetch(`${base_url}/models`, { signal: controller.signal });
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      return {
        running: false,
        base_url,
        model: "",
        error: `GET ${base_url}/models → ${res.status}: ${text.slice(0, 200)}`,
      };
    }
    const data = (await res.json()) as { data?: { id: string }[] };
    const models = (data.data ?? []).map((m) => m.id);
    return {
      running: true,
      base_url,
      model: "",
      models,
      latency_ms: Date.now() - started,
    };
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    return {
      running: false,
      base_url,
      model: "",
      error: `local server not reachable: ${msg}`,
    };
  } finally {
    clearTimeout(timer);
  }
}

function runCommand(
  command: string,
  opts: { cwd?: string; env?: Record<string, string> },
): Promise<{ exitCode: number; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, {
      shell: true,
      cwd: opts.cwd,
      env: { ...process.env, ...(opts.env ?? {}) },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout?.on("data", (d) => (stdout += d.toString()));
    child.stderr?.on("data", (d) => (stderr += d.toString()));
    child.on("error", (err) => reject(err));
    child.on("close", (code) => resolve({ exitCode: code ?? 0, stdout, stderr }));
  });
}

export interface StartResult {
  ok: boolean;
  profile: string;
  status: LocalStatus;
  launched?: { exitCode: number; stdout: string; stderr: string };
  error?: string;
}

export class LocalModelManager {
  constructor(private profiles: LocalModelProfile[] = DEFAULT_PROFILES) {}

  list(): LocalModelProfile[] {
    return this.profiles;
  }

  get(id: string): LocalModelProfile | undefined {
    return this.profiles.find((p) => p.id === id);
  }

  /** Status of the local server for a profile (health probe). */
  status(id: string, timeoutMs = 3000): Promise<LocalStatus> {
    const profile = this.get(id);
    if (!profile) return Promise.reject(new Error(`unknown local profile "${id}"`));
    return probe(profile.base_url, timeoutMs).then((s) => ({ ...s, model: profile.model }));
  }

  /**
   * Launch the server via the profile's `start` command (if any), then poll
   * until the endpoint reports healthy or `timeoutMs` elapses.
   */
  async start(
    id: string,
    opts: { timeoutMs?: number; pollMs?: number; launch?: boolean } = {},
  ): Promise<StartResult> {
    const profile = this.get(id);
    if (!profile) throw new Error(`unknown local profile "${id}"`);
    const timeoutMs = opts.timeoutMs ?? 3_600_000;
    const pollMs = opts.pollMs ?? 3_000;

    let launched: StartResult["launched"];
    if (opts.launch !== false) {
      if (!profile.start) {
        const s = await this.status(id);
        return {
          ok: s.running,
          profile: id,
          status: s,
          error: s.running
            ? undefined
            : `profile "${id}" has no start command and the server is not running. ` +
              `Run the recipe's launcher yourself, then re-run with a profile that sets "start".`,
        };
      }
      launched = await runCommand(profile.start!, { cwd: profile.cwd, env: profile.env });
      if (launched.exitCode !== 0) {
        return {
          ok: false,
          profile: id,
          status: { running: false, base_url: profile.base_url, model: profile.model },
          launched,
          error: `start command exited ${launched.exitCode}: ${launched.stderr.slice(0, 400)}`,
        };
      }
    }

    const deadline = Date.now() + timeoutMs;
    let status = await this.status(id);
    while (!status.running && Date.now() < deadline) {
      await new Promise((r) => setTimeout(r, pollMs));
      status = await this.status(id);
    }
    return { ok: status.running, profile: id, status, launched };
  }

  /** Stop the server via the profile's `stop` command (best-effort). */
  async stop(id: string): Promise<{ ok: boolean; profile: string; result?: { exitCode: number; stdout: string; stderr: string }; error?: string }> {
    const profile = this.get(id);
    if (!profile) throw new Error(`unknown local profile "${id}"`);
    if (!profile.stop) {
      return { ok: false, profile: id, error: `profile "${id}" has no stop command` };
    }
    const result = await runCommand(profile.stop, { cwd: profile.cwd, env: profile.env });
    return { ok: result.exitCode === 0, profile: id, result };
  }

  /**
   * Send a tiny chat completion to verify the model actually responds.
   * Uses the same OpenAI-compatible path the harness uses.
   */
  async test(
    id: string,
    opts: { prompt?: string; max_tokens?: number; timeoutMs?: number } = {},
  ): Promise<{ ok: boolean; profile: string; reply?: string; model?: string; latency_ms?: number; error?: string }> {
    const profile = this.get(id);
    if (!profile) throw new Error(`unknown local profile "${id}"`);
    const prompt = opts.prompt ?? "Reply with the single word: ok";
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), opts.timeoutMs ?? 120_000);
    const started = Date.now();
    try {
      const res = await fetch(`${profile.base_url}/chat/completions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          model: profile.model,
          messages: [{ role: "user", content: prompt }],
          max_tokens: opts.max_tokens ?? 24,
          temperature: 0,
          stream: false,
        }),
        signal: controller.signal,
      });
      if (!res.ok) {
        const text = await res.text().catch(() => "");
        return { ok: false, profile: id, error: `HTTP ${res.status}: ${text.slice(0, 300)}` };
      }
      const data = (await res.json()) as {
        choices?: { message?: { content?: string; reasoning?: string } }[];
        model?: string;
      };
      const msg = data.choices?.[0]?.message;
      const reply = msg?.content ?? msg?.reasoning ?? "";
      return {
        ok: reply.length > 0,
        profile: id,
        reply,
        model: data.model ?? profile.model,
        latency_ms: Date.now() - started,
      };
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      return { ok: false, profile: id, error: msg };
    } finally {
      clearTimeout(timer);
    }
  }
}

export function createLocalModelManager(profiles?: LocalModelProfile[]): LocalModelManager {
  return new LocalModelManager(profiles ?? DEFAULT_PROFILES);
}
