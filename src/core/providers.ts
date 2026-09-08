/**
 * Mythos Harness — LLM provider abstraction.
 *
 * We deliberately support:
 *  1. Any OpenAI-compatible HTTP endpoint (OpenAI, OpenRouter, vLLM,
 *     LM Studio, Ollama /v1, DeepSeek, Together, Groq, localAI, ...).
 *  2. A demo mode that returns a deterministic response so the harness
 *     works end-to-end without credentials.
 *
 * No API key is required to use the harness; set RHA_API_KEY (or
 * OPENAI_API_KEY) to talk to a real model.
 */

import type { ConversationMessage, ProviderConfig } from "./types.js";

export interface ChatCompletion {
  content: string;
  model: string;
  usage?: { prompt_tokens?: number; completion_tokens?: number };
  finish_reason?: string;
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

  if (kind === "demo" || !pickKey()) {
    return new DemoProvider(model);
  }

  return new OpenAICompatibleProvider({
    base_url,
    model,
    temperature,
    max_tokens,
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
}

class OpenAICompatibleProvider implements Provider {
  readonly kind = "openai-compatible" as const;
  constructor(private opts: OpenAICompatibleOptions) {}

  get model(): string {
    return this.opts.model;
  }

  private headers(): Record<string, string> {
    const key = pickKey();
    return {
      "content-type": "application/json",
      authorization: key ? `Bearer ${key}` : "",
    };
  }

  async chat(
    messages: ConversationMessage[],
    opts?: { temperature?: number; max_tokens?: number; signal?: AbortSignal },
  ): Promise<ChatCompletion> {
    const res = await fetch(`${this.opts.base_url}/chat/completions`, {
      method: "POST",
      headers: this.headers(),
      body: JSON.stringify({
        model: this.opts.model,
        messages: normalizeMessages(messages),
        temperature: opts?.temperature ?? this.opts.temperature,
        max_tokens: opts?.max_tokens ?? this.opts.max_tokens,
        stream: false,
      }),
      signal: opts?.signal,
    });

    if (!res.ok) {
      const body = await res.text().catch(() => "");
      throw new Error(`LLM provider returned ${res.status}: ${body.slice(0, 400)}`);
    }

    const data = (await res.json()) as {
      choices?: { message?: { content?: string }; finish_reason?: string }[];
      usage?: { prompt_tokens?: number; completion_tokens?: number };
      model?: string;
    };
    const content = data.choices?.[0]?.message?.content ?? "";
    return {
      content,
      model: data.model ?? this.opts.model,
      usage: data.usage,
      finish_reason: data.choices?.[0]?.finish_reason,
    };
  }

  async stream(
    messages: ConversationMessage[],
    onDelta: (d: string) => void,
    opts?: { temperature?: number; max_tokens?: number; signal?: AbortSignal },
  ): Promise<ChatCompletion> {
    const res = await fetch(`${this.opts.base_url}/chat/completions`, {
      method: "POST",
      headers: { ...this.headers(), accept: "text/event-stream" },
      body: JSON.stringify({
        model: this.opts.model,
        messages: normalizeMessages(messages),
        temperature: opts?.temperature ?? this.opts.temperature,
        max_tokens: opts?.max_tokens ?? this.opts.max_tokens,
        stream: true,
      }),
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
            choices?: { delta?: { content?: string }; finish_reason?: string }[];
          };
          if (json.model) model = json.model;
          const delta = json.choices?.[0]?.delta?.content ?? "";
          if (delta) {
            full += delta;
            onDelta(delta);
          }
          if (json.choices?.[0]?.finish_reason) finish_reason = json.choices[0].finish_reason;
        } catch {
          // Ignore partial frames
        }
      }
    }

    return { content: full, model, finish_reason };
  }
}
