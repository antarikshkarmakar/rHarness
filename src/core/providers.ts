/**
 * rHarness — LLM provider abstraction.
 *
 * We deliberately support:
 *  1. Any OpenAI-compatible HTTP endpoint — including **locally-served models**:
 *     vLLM / tabbyAPI serving EXL3 weights, llama.cpp / Ollama / LM Studio serving
 *     GGUF, and NVFP4 checkpoints. These are all driven through the same
 *     `/v1/chat/completions` surface and typically need no API key.
 *  2. Cloud OpenAI-compatible endpoints (OpenAI, OpenRouter, DeepSeek, Together,
 *     Groq, ...) via an API key.
 *  3. A demo mode that returns a deterministic response so the harness works
 *     end-to-end without any model.
 *
 * No API key is required for a local endpoint. For GLM-5.3-style chat templates
 * (thinking toggle, `reasoning` field) see `buildRequestBody` / `ChatCompletion.reasoning`.
 */

import type { ConversationMessage, ProviderConfig } from "./types.js";

export interface ChatCompletion {
  content: string;
  model: string;
  usage?: { prompt_tokens?: number; completion_tokens?: number };
  finish_reason?: string;
  /** Chain-of-thought (GLM-5.3 `reasoning`). Absent for non-reasoning models. */
  reasoning?: string;
}

export interface Provider {
  readonly kind: ProviderConfig["kind"];
  readonly model: string;
  chat(messages: ConversationMessage[], opts?: { temperature?: number; max_tokens?: number; signal?: AbortSignal }): Promise<ChatCompletion>;
  stream(
    messages: ConversationMessage[],
    onDelta: (delta: string) => void,
    opts?: { temperature?: number; max_tokens?: number; signal?: AbortSignal },
  ): Promise<ChatCompletion>;
}

function normalizeMessages(messages: ConversationMessage[]): { role: string; content: string }[] {
  return messages.map((m) => ({ role: m.role, content: m.content }));
}

function pickKey(): string | undefined {
  return (
    process.env.RHA_API_KEY ||
    process.env.OPENAI_API_KEY ||
    process.env.LLMAPI_KEY ||
    undefined
  );
}

function pickBaseUrl(): string {
  const v =
    process.env.RHA_BASE_URL ||
    process.env.OPENAI_BASE_URL ||
    process.env.LLMAPI_BASE_URL ||
    "https://api.openai.com/v1";
  return v.replace(/\/+$/, "");
}

function pickModel(): string {
  return process.env.RHA_MODEL || process.env.OPENAI_MODEL || "gpt-4o-mini";
}

export function buildProvider(config?: Partial<ProviderConfig>): Provider {
  const kind = config?.kind ?? (pickKey() ? "openai-compatible" : "demo");
  const base_url = config?.base_url ?? pickBaseUrl();
  const model = config?.model ?? pickModel();
  const temperature = config?.temperature ?? 0.2;
  const max_tokens = config?.max_tokens ?? 1024;
  const api_key = config?.api_key ?? pickKey();
  const extra_body = config?.extra_body;
  const reasoning_effort = config?.reasoning_effort;
  const enable_thinking = config?.enable_thinking;
  const top_p = config?.top_p;
  const top_k = config?.top_k;
  const max_context_length = config?.max_context_length;
  const gpu_offload_layers = config?.gpu_offload_layers;
  const cpu_threads = config?.cpu_threads;
  const flash_attention = config?.flash_attention;
  const response_format = config?.response_format;

  // A local/unauthenticated endpoint (vLLM, tabbyAPI, Ollama) has no key.
  // If a base_url points at a local host and no key is configured, we still
  // use the OpenAI-compatible provider rather than silently dropping to demo.
  const looksLocal = /^(127\.|localhost|0\.0\.0\.0|\[::1\])/.test(base_url.replace(/^https?:\/\//, ""));
  if (kind === "demo" || (!api_key && !looksLocal)) {
    return new DemoProvider(model);
  }

  return new OpenAICompatibleProvider({
    base_url,
    model,
    temperature,
    max_tokens,
    api_key,
    extra_body,
    reasoning_effort,
    enable_thinking,
    top_p,
    top_k,
    max_context_length,
    gpu_offload_layers,
    cpu_threads,
    flash_attention,
    response_format,
  });
}

class DemoProvider implements Provider {
  readonly kind = "demo" as const;
  constructor(public model: string) {}

  async chat(messages: ConversationMessage[]): Promise<ChatCompletion> {
    const last = messages[messages.length - 1];
    const content = [
      `[rHarness — demo mode]`,
      ``,
      `I don't have an API key configured, so I'm answering with a`,
      `deterministic scaffold. Set RHA_API_KEY (or OPENAI_API_KEY) and`,
      `optionally RHA_BASE_URL / RHA_MODEL to talk to a real model.`,
      ``,
      `Your latest message:`,
      `> ${last?.content?.slice(0, 400) ?? "(empty)"}`,
      ``,
      `In production this slot would contain a full finish-first plan`,
      `covering Interrogate → Contract → Execute → Finish.`,
    ].join("\n");
    return {
      content,
      model: this.model,
      finish_reason: "stop",
    };
  }

  async stream(
    messages: ConversationMessage[],
    onDelta: (d: string) => void,
  ): Promise<ChatCompletion> {
    const out = await this.chat(messages);
    // Emit in small chunks to mimic streaming.
    const chunks = out.content.match(/.{1,24}/gs) ?? [out.content];
    for (const c of chunks) onDelta(c);
    return out;
  }
}

interface OpenAICompatibleOptions {
  base_url: string;
  model: string;
  temperature: number;
  max_tokens: number;
  api_key?: string;
  extra_body?: Record<string, unknown>;
  reasoning_effort?: "low" | "high";
  /** GLM-5.3-style thinking toggle → `chat_template_kwargs.enable_thinking`. */
  enable_thinking?: boolean;
  top_p?: number;
  top_k?: number;
  max_context_length?: number;
  gpu_offload_layers?: number;
  cpu_threads?: number;
  flash_attention?: boolean;
  response_format?: { type: "json_object" } | { type: "json_schema"; json_schema: Record<string, unknown> };
}

/**
 * Build the request body shared by chat() and stream().
 *
 * Handles the GLM-5.3-style chat template: when `enable_thinking` is set (from
 * config or env) it is folded into `chat_template_kwargs` so the local server
 * can disable/enable reasoning. `extra_body` fields are merged verbatim and win
 * over the computed defaults.
 */
function buildRequestBody(
  opts: OpenAICompatibleOptions,
  messages: ConversationMessage[],
  per: { temperature?: number; max_tokens?: number; stream: boolean },
): Record<string, unknown> {
  const body: Record<string, unknown> = {
    model: opts.model,
    messages: normalizeMessages(messages),
    temperature: per.temperature ?? opts.temperature,
    max_tokens: per.max_tokens ?? opts.max_tokens,
    stream: per.stream,
  };
  if (opts.reasoning_effort) body.reasoning_effort = opts.reasoning_effort;

  // Fold enable_thinking into chat_template_kwargs (GLM-5.3 chat template reads it there).
  // Priority: provider config value (from defaultConfig / .env) → RHA_ENABLE_THINKING env.
  const envThinking = process.env.RHA_ENABLE_THINKING;
  const enableThinking =
    opts.enable_thinking !== undefined
      ? opts.enable_thinking
      : envThinking !== undefined && envThinking !== ""
        ? envThinking === "1" || envThinking.toLowerCase() === "true"
        : undefined;
  const mergedChatTemplate: Record<string, unknown> = {
    ...((opts.extra_body?.chat_template_kwargs as Record<string, unknown>) ?? {}),
  };
  if (enableThinking !== undefined) mergedChatTemplate.enable_thinking = enableThinking;
  // Inference tuning — forwarded as top-level fields; servers that don't support them ignore them.
  if (opts.top_p !== undefined) body.top_p = opts.top_p;
  if (opts.top_k !== undefined) body.top_k = opts.top_k;
  if (opts.max_context_length !== undefined) body.max_model_len = opts.max_context_length;
  if (opts.gpu_offload_layers !== undefined) body.n_gpu_layers = opts.gpu_offload_layers;
  if (opts.cpu_threads !== undefined) body.n_threads = opts.cpu_threads;
  if (opts.flash_attention !== undefined) body.use_flash_attn = opts.flash_attention;
  if (opts.response_format !== undefined) body.response_format = opts.response_format;
  const extraBody = { ...(opts.extra_body ?? {}) };
  if (Object.keys(mergedChatTemplate).length > 0) extraBody.chat_template_kwargs = mergedChatTemplate;
  Object.assign(body, extraBody);
  return body;
}

class OpenAICompatibleProvider implements Provider {
  readonly kind = "openai-compatible" as const;
  constructor(private opts: OpenAICompatibleOptions) {}

  get model(): string {
    return this.opts.model;
  }

  private headers(): Record<string, string> {
    const key = this.opts.api_key ?? pickKey();
    return {
      "content-type": "application/json",
      ...(key ? { authorization: `Bearer ${key}` } : {}),
    };
  }

  /**
   * Extract assistant text from a (non-stream) message. GLM-5.3 models may put
   * the answer in `content` and the chain-of-thought in `reasoning`; we surface
   * both but prefer non-empty `content`, falling back to `reasoning`.
   */
  private readMessage(message?: { content?: string; reasoning?: string; reasoning_content?: string }): {
    content: string;
    reasoning: string;
  } {
    const reasoning = message?.reasoning ?? message?.reasoning_content ?? "";
    const content = message?.content ?? "";
    return { content: content || reasoning, reasoning };
  }

  async chat(
    messages: ConversationMessage[],
    opts?: { temperature?: number; max_tokens?: number; signal?: AbortSignal },
  ): Promise<ChatCompletion> {
    const res = await fetch(`${this.opts.base_url}/chat/completions`, {
      method: "POST",
      headers: this.headers(),
      body: JSON.stringify(buildRequestBody(this.opts, messages, { ...opts, stream: false })),
      signal: opts?.signal,
    });

    if (!res.ok) {
      const body = await res.text().catch(() => "");
      throw new Error(`LLM provider returned ${res.status}: ${body.slice(0, 400)}`);
    }

    const data = (await res.json()) as {
      choices?: { message?: { content?: string; reasoning?: string; reasoning_content?: string }; finish_reason?: string }[];
      usage?: { prompt_tokens?: number; completion_tokens?: number };
      model?: string;
    };
    const { content, reasoning } = this.readMessage(data.choices?.[0]?.message);
    return {
      content,
      model: data.model ?? this.opts.model,
      usage: data.usage,
      finish_reason: data.choices?.[0]?.finish_reason,
      ...(reasoning ? { reasoning } : {}),
    } as ChatCompletion;
  }

  async stream(
    messages: ConversationMessage[],
    onDelta: (d: string) => void,
    opts?: { temperature?: number; max_tokens?: number; signal?: AbortSignal },
  ): Promise<ChatCompletion> {
    const res = await fetch(`${this.opts.base_url}/chat/completions`, {
      method: "POST",
      headers: { ...this.headers(), accept: "text/event-stream" },
      body: JSON.stringify(buildRequestBody(this.opts, messages, { ...opts, stream: true })),
      signal: opts?.signal,
    });

    if (!res.ok || !res.body) {
      const body = await res.text().catch(() => "");
      throw new Error(`LLM provider (stream) returned ${res.status}: ${body.slice(0, 400)}`);
    }

    // Read SSE stream
    const reader = (res.body as ReadableStream<Uint8Array>).getReader();
    const decoder = new TextDecoder("utf-8");
    let buf = "";
    let full = "";
    let reasoning = "";
    let finish_reason: string | undefined;
    let model = this.opts.model;

    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const lines = buf.split("\n");
      buf = lines.pop() ?? "";
      for (const raw of lines) {
        const line = raw.trim();
        if (!line.startsWith("data:")) continue;
        const payload = line.slice(5).trim();
        if (payload === "[DONE]") continue;
        try {
          const json = JSON.parse(payload) as {
            model?: string;
            choices?: {
              delta?: { content?: string; reasoning?: string; reasoning_content?: string };
              finish_reason?: string;
            }[];
          };
          if (json.model) model = json.model;
          const delta = json.choices?.[0]?.delta;
          const contentDelta = delta?.content ?? "";
          const reasoningDelta = delta?.reasoning ?? delta?.reasoning_content ?? "";
          if (reasoningDelta) reasoning += reasoningDelta;
          if (contentDelta) {
            full += contentDelta;
            onDelta(contentDelta);
          }
          if (json.choices?.[0]?.finish_reason) finish_reason = json.choices[0].finish_reason;
        } catch {
          // Ignore partial frames
        }
      }
    }

    return {
      content: full || reasoning,
      model,
      finish_reason,
      ...(reasoning ? { reasoning } : {}),
    } as ChatCompletion;
  }
}
