/**
 * Mythos Harness — Web Search plugin.
 *
 * Uses DuckDuckGo's HTML endpoint (no API key required). If DDG is
 * unreachable, the plugin returns a graceful "offline" result instead of
 * throwing — the harness loop can continue and let other plugins do work.
 */

import type { Phase, PhaseContext, PhaseResult, Plugin } from "../core/types.js";
import { nowIso } from "../core/types.js";

interface SearchHit {
  title: string;
  url: string;
  snippet: string;
  rank: number;
  source: string;
}

async function searchDuckDuckGo(query: string, maxResults: number): Promise<SearchHit[]> {
  const url = "https://html.duckduckgo.com/html/";
  const body = new URLSearchParams({ q: query, kl: "us-en" });
  const res = await fetch(url, {
    method: "POST",
    headers: {
      "content-type": "application/x-www-form-urlencoded",
      "user-agent": "Mozilla/5.0 (X11; Linux x86_64) MythosHarness/1.0",
    },
    body: body.toString(),
    signal: AbortSignal.timeout(10_000),
  });
  if (!res.ok) throw new Error(`DDG returned ${res.status}`);
  const html = await res.text();

  // Lightweight HTML parse — avoids pulling in a full DOM dep for v1.
  const hits: SearchHit[] = [];
  const re = /<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>([\s\S]*?)<\/a>[\s\S]*?<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>([\s\S]*?)<\/a>/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html)) !== null && hits.length < maxResults) {
    const urlStr = decodeURIComponent(m[1] ?? "");
    const rawTitle = m[2] ?? "";
    const rawSnippet = m[3] ?? "";
    const title = rawTitle.replace(/<[^>]+>/g, "").replace(/\s+/g, " ").trim();
    const snippet = rawSnippet.replace(/<[^>]+>/g, "").replace(/\s+/g, " ").trim();
    if (title || urlStr) {
      hits.push({
        title,
        url: urlStr,
        snippet,
        rank: hits.length,
        source: "duckduckgo",
      });
    }
  }

  return hits;
}

export const webSearchPlugin: Plugin = {
  id: "web-search",
  name: "Web Search",
  version: "1.0.0",
  description: "Search the web (DuckDuckGo HTML) and return ranked results with snippets.",
  capabilities: {
    phases: ["Interrogate", "Contract", "Execute"],
    tools: ["web.search"],
  },

  async execute(phase: Phase, ctx: PhaseContext): Promise<PhaseResult> {
    const started = nowIso();
    const queries = (ctx.data?.web_queries as string[]) ?? [];
    const errors: string[] = [];
    const outputs: Record<string, SearchHit[]> = {};
    const maxResults = Math.min(
      Number(ctx.data?.web_max_results ?? 5),
      10,
    );

    for (const q of queries) {
      try {
        outputs[q] = await searchDuckDuckGo(q, maxResults);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        errors.push(`search failed for "${q}": ${msg}`);
        outputs[q] = [];
      }
    }

    return {
      phase,
      success: errors.length === 0,
      output: { phase, queries: queries.length, results: outputs },
      errors,
      artifacts: [],
      started_at: started,
      completed_at: nowIso(),
      duration_ms: 0,
    };
  },
};
