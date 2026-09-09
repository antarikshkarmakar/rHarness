/**
 * Mythos Harness — Core type definitions.
 *
 * These types mirror the Rust workspace (crates/core/src/types.rs) and form
 * the contract between the loop, plugins, agents, providers, and the web UI.
 */

export const PHASES = ["Interrogate", "Contract", "Execute", "Finish"] as const;
export type Phase = (typeof PHASES)[number];

export type LoopStrategy = "finish-first";

export type RunStatus = "idle" | "running" | "completed" | "failed" | "cancelled";

export interface SuccessCriteria {
  min_confidence: number;
  required_artifacts: string[];
  max_retries: number;
}

export interface Artifact {
  id: string;
  name: string;
  kind: string; // e.g. "file" | "url" | "snippet" | "output"
  path?: string;
  url?: string;
  content?: string;
  created_at: string; // ISO-8601
  phase?: Phase;
}

export interface PluginResult {
  phase: Phase;
  success: boolean;
  output: unknown;
  artifacts?: Artifact[];
  errors?: string[];
  started_at: string;
  completed_at: string;
  duration_ms: number;
}

export interface PhaseResult extends PluginResult {
  phase: Phase;
}

/** Mutable bag of shared state threaded through the finish-first loop. */
export interface PhaseContext {
  task: string;
  conversation: ConversationMessage[];
  contract?: Contract;
  data: Record<string, unknown>;
  artifacts: Artifact[];
  logs: string[];
  iterations: number;
  max_iterations: number;
  working_dir: string;
  signal?: AbortSignal;
}

export interface Contract {
  goals: string[];
  non_goals: string[];
  success_criteria: string;
  assumptions: string[];
  risks: string[];
  plan: string[];
}

export interface PluginCapabilities {
  phases: Phase[];
  tools: string[];
}

/**
 * A plugin contributes behavior to one or more phases of the loop.
 * Plugins are the primary extension mechanism of the harness.
 */
export interface Plugin {
  id: string;
  name: string;
  version: string;
  description: string;
  capabilities: PluginCapabilities;
  /** Optional lifecycle hooks */
  init?(ctx: PhaseContext): Promise<void> | void;
  dispose?(): Promise<void> | void;
  /**
   * Core hook: do work for a phase, given the current context.
   * Return a PluginResult. Errors should be caught and returned as
   * PluginResult.errors, not thrown.
   */
  execute(phase: Phase, ctx: PhaseContext): Promise<PluginResult> | PluginResult;
}

export interface Agent {
  id: string;
  name: string;
  version: string;
  loop: LoopStrategy;
  success_criteria: SuccessCriteria;
  run(phase: Phase, ctx: PhaseContext): Promise<PhaseResult>;
}

export interface ConversationMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  name?: string;
  timestamp: string;
}

export type HarnessEventType =
  | "run.started"
  | "phase.started"
  | "phase.completed"
  | "phase.failed"
  | "artifact.created"
  | "plugin.output"
  | "loop.iteration"
  | "run.completed"
  | "run.failed"
  | "chat.delta"
  | "chat.completed"
  | "chat.error";

export interface HarnessEvent {
  type: HarnessEventType;
  ts: string;
  run_id: string;
  data: Record<string, unknown>;
}

export interface HarnessRun {
  id: string;
  task: string;
  status: RunStatus;
  phases: PhaseResult[];
  artifacts: Artifact[];
  events: HarnessEvent[];
  started_at?: string;
  completed_at?: string;
  confidence?: number;
  summary?: string;
}

export interface HarnessConfig {
  name: string;
  version: string;
  tagline: string;
  loop_strategy: LoopStrategy;
  phases: Phase[];
  max_iterations: number;
  phase_timeout_ms: number;
  working_dir: string;
  success_criteria: SuccessCriteria;
  provider: ProviderConfig;
}

export interface ProviderConfig {
  kind: "openai-compatible" | "demo";
  base_url?: string;
  model?: string;
  temperature?: number;
  max_tokens?: number;
  /** Optional bearer key. Empty/absent is fine for unauthenticated local servers (vLLM, tabbyAPI). */
  api_key?: string;
  /**
   * Extra top-level fields merged verbatim into every chat-completion request.
   * Used for GLM-5.3-style chat templates, e.g. `{ chat_template_kwargs: { enable_thinking: false } }`.
   */
  extra_body?: Record<string, unknown>;
  /** GLM-5.3: enable/disable thinking. Mapped into `chat_template_kwargs.enable_thinking`. */
  enable_thinking?: boolean;
  /** GLM-5.3: `low` | `high` (or unset = Max). Sent as top-level `reasoning_effort`. */
  reasoning_effort?: "low" | "high";
  /** Sampling: restrict token selection to top cumulative probability (0–1). Forwarded as `top_p`. */
  top_p?: number;
  /** Sampling: restrict to top K candidate tokens (EXL3 / llama.cpp). Forwarded as `top_k`. */
  top_k?: number;
  /** System prompt / role definition applied to every conversation. */
  system_prompt?: string;
  /** Maximum context window in tokens (input + output). Forwarded as `max_model_len`. */
  max_context_length?: number;
  /** Number of layers offloaded to GPU (-1 = all layers). Forwarded as `n_gpu_layers`. */
  gpu_offload_layers?: number;
  /** CPU thread pool size for non-GPU work. Forwarded as `n_threads`. */
  cpu_threads?: number;
  /** Enable Flash Attention for lower memory and faster inference. Forwarded as `use_flash_attn`. */
  flash_attention?: boolean;
  /** Force structured JSON output. `json_object` or `json_schema`. */
  response_format?: { type: "json_object" } | { type: "json_schema"; json_schema: Record<string, unknown> };
}

export interface Harness {
  config: HarnessConfig;
  registry: PluginRegistry;
  runTask(task: string, opts?: RunOptions): Promise<HarnessRun>;
  chat(messages: ConversationMessage[], opts?: RunOptions): AsyncGenerator<HarnessEvent, void, unknown>;
  state(): { runs: HarnessRun[]; plugins: PluginInfo[] };
}

export interface RunOptions {
  working_dir?: string;
  signal?: AbortSignal;
  max_iterations?: number;
}

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  capabilities: PluginCapabilities;
  enabled: boolean;
}

export class PluginRegistry {
  private plugins = new Map<string, Plugin>();
  private enabled = new Map<string, boolean>();

  register(plugin: Plugin): void {
    if (this.plugins.has(plugin.id)) {
      this.unregister(plugin.id);
    }
    this.plugins.set(plugin.id, plugin);
    this.enabled.set(plugin.id, true);
  }

  unregister(id: string): void {
    const p = this.plugins.get(id);
    if (p) {
      try {
        void p.dispose?.();
      } catch {
        // ignore disposal errors
      }
    }
    this.plugins.delete(id);
    this.enabled.delete(id);
  }

  get(id: string): Plugin | undefined {
    return this.plugins.get(id);
  }

  all(): Plugin[] {
    return Array.from(this.plugins.values());
  }

  enabledPlugins(): Plugin[] {
    return this.all().filter((p) => this.enabled.get(p.id));
  }

  forPhase(phase: Phase): Plugin[] {
    return this.enabledPlugins().filter((p) => p.capabilities.phases.includes(phase));
  }

  setEnabled(id: string, enabled: boolean): boolean {
    if (!this.plugins.has(id)) return false;
    this.enabled.set(id, enabled);
    return true;
  }

  isEnabled(id: string): boolean {
    return this.enabled.get(id) ?? false;
  }

  info(): PluginInfo[] {
    return this.all().map((p) => ({
      id: p.id,
      name: p.name,
      version: p.version,
      description: p.description,
      capabilities: p.capabilities,
      enabled: this.isEnabled(p.id),
    }));
  }
}

export function nowIso(): string {
  return new Date().toISOString();
}

export function uuid(): string {
  return crypto.randomUUID();
}
