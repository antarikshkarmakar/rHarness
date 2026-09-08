/**
 * Mythos Harness — finish-first loop.
 *
 * The loop runs the four phases in order and, when the Finish phase
 * reports "not finished", loops back to Interrogate with the accumulated
 * context until success criteria are met or max_iterations is reached.
 *
 * This is the heart of the product: the same task can be attempted again
 * — with more context and better judgment — until it is actually done.
 */

import type {
  Agent,
  Contract,
  HarnessConfig,
  HarnessEvent,
  HarnessRun,
  Phase,
  PhaseContext,
  PhaseResult,
  PluginRegistry,
  RunOptions,
} from "./types.js";
import { PHASES, nowIso, uuid } from "./types.js";
import { buildProvider, type Provider } from "./providers.js";

export interface LoopDeps {
  registry: PluginRegistry;
  agent?: Agent;
  provider: Provider;
  config: HarnessConfig;
}

export async function newContext(
  task: string,
  config: HarnessConfig,
  opts?: RunOptions,
): Promise<PhaseContext> {
  const ctx: PhaseContext = {
    task,
    conversation: [
      {
        role: "system",
        content:
          "You are rHarness, a finish-first autonomous agent. " +
          "You work in four phases: Interrogate, Contract, Execute, Finish. " +
          "Always aim to actually finish the task, not merely talk about it.",
        timestamp: nowIso(),
      },
      {
        role: "user",
        content: task,
        timestamp: nowIso(),
      },
    ],
    data: {},
    artifacts: [],
    logs: [],
    iterations: 0,
    max_iterations: opts?.max_iterations ?? config.max_iterations,
    working_dir: opts?.working_dir ?? config.working_dir,
    signal: opts?.signal,
  };
  return ctx;
}

interface PhaseOutcome {
  result: PhaseResult;
  events: HarnessEvent[];
}

/**
 * Run a single phase: invoke the LLM to reason about it, then hand the
 * context to every enabled plugin for that phase, and finally merge the
 * plugin outputs back into the PhaseResult.
 */
async function runPhase(
  phase: Phase,
  ctx: PhaseContext,
  deps: LoopDeps,
  runId: string,
): Promise<PhaseOutcome> {
  const startedAt = nowIso();
  const startedMs = Date.now();
  const events: HarnessEvent[] = [
    { type: "phase.started", ts: startedAt, run_id: runId, data: { phase } },
  ];

  ctx.logs.push(`[phase:${phase}] started`);

  // 1. LLM reasoning for the phase
  let llmText = "";
  try {
    const messages = [
      ...ctx.conversation,
      {
        role: "assistant" as const,
        content:
          `Phase: ${phase}\n` +
          `Current iteration: ${ctx.iterations + 1}/${ctx.max_iterations}\n` +
          `Task: ${ctx.task}\n` +
          (ctx.contract ? `Contract:\n${JSON.stringify(ctx.contract, null, 2)}\n` : "") +
          `Artifacts so far:\n${ctx.artifacts.map((a) => `- ${a.name} (${a.kind})`).join("\n")}\n\n` +
          `Produce a concise, concrete output for the "${phase}" phase.`,
        timestamp: nowIso(),
      },
    ];
    const out = await deps.provider.chat(messages, { signal: ctx.signal });
    llmText = out.content;
    ctx.conversation.push({
      role: "assistant",
      name: `phase:${phase}`,
      content: llmText,
      timestamp: nowIso(),
    });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    ctx.logs.push(`[phase:${phase}] provider error: ${msg}`);
    // Continue — plugins may still be able to make progress.
  }

  // 2. Run enabled plugins for this phase
  const plugins = deps.registry.forPhase(phase);
  const errors: string[] = [];
  let success = llmText.length > 0 || plugins.length === 0;
  const pluginOutputs: Record<string, unknown> = {};
  const artifacts = [...ctx.artifacts];

  for (const plugin of plugins) {
    if (ctx.signal?.aborted) {
      errors.push(`aborted before plugin ${plugin.id}`);
      break;
    }
    try {
      const res = await plugin.execute(phase, ctx);
      if (!res.success) success = false;
      if (res.errors?.length) errors.push(...res.errors.map((e) => `[${plugin.id}] ${e}`));
      if (res.artifacts?.length) {
        artifacts.push(...res.artifacts);
        for (const a of res.artifacts) {
          events.push({
            type: "artifact.created",
            ts: nowIso(),
            run_id: runId,
            data: { artifact: a },
          });
        }
      }
      pluginOutputs[plugin.id] = res.output;
      ctx.artifacts = artifacts;
      events.push({
        type: "plugin.output",
        ts: nowIso(),
        run_id: runId,
        data: { phase, plugin: plugin.id, success: res.success },
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      success = false;
      errors.push(`[${plugin.id}] ${msg}`);
    }
  }

  const completedAt = nowIso();
  const durationMs = Date.now() - startedMs;

  // 3. Compose PhaseResult
  const result: PhaseResult = {
    phase,
    success,
    output: {
      llm: llmText as string,
      plugins: pluginOutputs as Record<string, unknown>,
    },
    errors,
    artifacts,
    started_at: startedAt,
    completed_at: completedAt,
    duration_ms: durationMs,
  };

  events.push({
    type: success ? "phase.completed" : "phase.failed",
    ts: completedAt,
    run_id: runId,
    data: { phase, success, errors: errors.length },
  });

  ctx.logs.push(`[phase:${phase}] ${success ? "completed" : "failed"} in ${durationMs}ms`);
  return { result, events };
}

/**
 * Decide whether the loop should continue after the Finish phase.
 * In demo mode we always trust the model's verdict unless artifacts are
 * explicitly required and missing.
 */
function shouldStopAfterFinish(result: PhaseResult, ctx: PhaseContext, config: HarnessConfig): boolean {
  if (ctx.iterations >= ctx.max_iterations) return true;
  if (ctx.signal?.aborted) return true;
  if (!result.success) return false;

  // If we promised artifacts and have none, keep iterating.
  if (config.success_criteria.required_artifacts.length > 0 && ctx.artifacts.length === 0) {
    return false;
  }
  return true;
}

export async function runFinishFirstLoop(
  task: string,
  deps: LoopDeps,
  opts?: RunOptions,
): Promise<HarnessRun> {
  const runId = uuid();
  const startedAt = nowIso();
  const events: HarnessEvent[] = [
    { type: "run.started", ts: startedAt, run_id: runId, data: { task } },
  ];

  const ctx = await newContext(task, deps.config, opts);

  let lastFinish: PhaseResult | undefined;
  let overallSuccess = false;

  while (ctx.iterations < ctx.max_iterations) {
    if (ctx.signal?.aborted) break;
    ctx.iterations += 1;
    events.push({
      type: "loop.iteration",
      ts: nowIso(),
      run_id: runId,
      data: { iteration: ctx.iterations },
    });

    // Interrogate → Contract → Execute → Finish
    for (const phase of deps.config.phases) {
      if (ctx.signal?.aborted) break;
      const { result, events: evts } = await runPhase(phase, ctx, deps, runId);
      events.push(...evts);

      if (phase === "Contract") {
        const out = result.output as { llm?: unknown };
        if (typeof out?.llm === "string") {
          ctx.contract = deriveContractFromText(ctx, out.llm);
        }
      }

      if (phase === "Finish") lastFinish = result;
    }

    // After a full cycle, decide whether we're truly finished.
    if (lastFinish && shouldStopAfterFinish(lastFinish, ctx, deps.config)) {
      overallSuccess = lastFinish.success;
      break;
    }
  }

  const completedAt = nowIso();
  const run: HarnessRun = {
    id: runId,
    task,
    status: ctx.signal?.aborted ? "cancelled" : overallSuccess ? "completed" : "failed",
    phases: [
      // Flatten: keep the last outcome per phase for observability.
    ],
    artifacts: ctx.artifacts,
    events,
    started_at: startedAt,
    completed_at: completedAt,
    summary: buildSummary(ctx),
    confidence: overallSuccess ? 0.9 : 0.4,
  };

  events.push({
    type: overallSuccess ? "run.completed" : "run.failed",
    ts: completedAt,
    run_id: runId,
    data: { success: overallSuccess, iterations: ctx.iterations },
  });

  return run;
}

function deriveContractFromText(ctx: PhaseContext, text: string): Contract {
  // Best-effort: split numbered lines as goals, everything else as assumptions.
  const goals = text
    .split(/\r?\n/)
    .map((l) => l.trim().replace(/^[-*\d.]+\s*/, ""))
    .filter((l) => l.length > 0)
    .slice(0, 8);
  return {
    goals,
    non_goals: [],
    success_criteria: "Task is demonstrably finished and verified.",
    assumptions: [`Operating on task: ${ctx.task}`],
    risks: [],
    plan: goals,
  };
}

function buildSummary(ctx: PhaseContext): string {
  const lines = [
    `Iterations: ${ctx.iterations}`,
    `Artifacts: ${ctx.artifacts.length}`,
    `Plugins: ${ctx.data?.plugins ?? "n/a"}`,
  ];
  if (ctx.contract) {
    lines.push(`Contract goals: ${ctx.contract.goals.length}`);
  }
  return lines.join("\n");
}

/**
 * Convenience factory that wires up the loop with a default provider.
 */
export function createLoop(
  registry: PluginRegistry,
  config: HarnessConfig,
  provider?: Provider,
) {
  const deps: LoopDeps = {
    registry,
    provider: provider ?? buildProvider(config.provider),
    config,
  };
  return {
    run: (task: string, opts?: RunOptions) => runFinishFirstLoop(task, deps, opts),
  };
}

export { PHASES };
