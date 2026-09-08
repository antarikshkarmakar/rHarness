/**
 * Mythos Harness — Harness facade.
 *
 * This is the single object the CLI / web server / SDK users interact with.
 * It composes a PluginRegistry, a Provider, and the finish-first loop.
 */

import { buildProvider, type Provider } from "./providers.js";
import { createLoop } from "./loop.js";
import type {
  ConversationMessage,
  Harness,
  HarnessConfig,
  HarnessEvent,
  HarnessRun,
  PhaseContext,
  PluginRegistry,
  RunOptions,
} from "./types.js";
import { PHASES, PluginRegistry as PR, nowIso, uuid } from "./types.js";
// (PhaseContext imported above for re-export only)

export interface HarnessOptions {
  config?: Partial<HarnessConfig>;
  registry?: PluginRegistry;
  provider?: Provider;
}

export function defaultConfig(): HarnessConfig {
  return {
    name: "rHarness",
    version: "1.0.0",
    tagline: "Finish First.",
    loop_strategy: "finish-first",
    phases: [...PHASES],
    max_iterations: 3,
    phase_timeout_ms: 120_000,
    working_dir: process.cwd(),
    success_criteria: {
      min_confidence: 0.8,
      required_artifacts: [],
      max_retries: 3,
    },
    provider: {
      kind: "openai-compatible",
      base_url: process.env.RHA_BASE_URL || process.env.OPENAI_BASE_URL,
      model: process.env.RHA_MODEL || process.env.OPENAI_MODEL,
      temperature: 0.2,
      // GLM-5.3-style long-context reasoning runs well past 8k tokens; keep the
      // cap high enough that it is not truncated mid-thought.
      max_tokens: Number(process.env.RHA_MAX_TOKENS || 32768),
      api_key: process.env.RHA_API_KEY || process.env.OPENAI_API_KEY,
      enable_thinking:
        process.env.RHA_ENABLE_THINKING !== undefined
          ? process.env.RHA_ENABLE_THINKING === "1" || process.env.RHA_ENABLE_THINKING.toLowerCase() === "true"
          : undefined,
      reasoning_effort: (process.env.RHA_REASONING_EFFORT as "low" | "high" | undefined) || undefined,
    },
  };
}

export function createHarness(options: HarnessOptions = {}): Harness {
  const config: HarnessConfig = { ...defaultConfig(), ...options.config };
  const registry = options.registry ?? new PR();
  const provider = options.provider ?? buildProvider(config.provider);

  const loop = createLoop(registry, config, provider);
  const runs: HarnessRun[] = [];

  async function* chat(
    messages: ConversationMessage[],
    opts?: RunOptions,
  ): AsyncGenerator<HarnessEvent, void, unknown> {
    const runId = uuid();
    const base = (
      type: HarnessEvent["type"],
      data: Record<string, unknown> = {},
    ): HarnessEvent => ({ type, ts: nowIso(), run_id: runId, data });

    yield base("run.started", { task: "chat" });

    try {
      const out = await provider.chat(messages, { signal: opts?.signal });
      yield base("chat.completed", {
        content: out.content,
        model: out.model,
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      yield base("chat.error", { error: msg });
    }
  }

  return {
    config,
    registry,
    async runTask(task: string, opts?: RunOptions) {
      const run = await loop.run(task, opts);
      runs.push(run);
      if (runs.length > 100) runs.splice(0, runs.length - 100);
      return run;
    },
    chat,
    state: () => ({
      runs: [...runs],
      plugins: registry.info(),
    }),
  };
}

export type { PhaseContext };
