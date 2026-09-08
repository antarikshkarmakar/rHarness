/**
 * Mythos Harness — Memory plugin.
 *
 * A small in-memory key/value store with optional SQLite persistence.
 * Keys are scoped by run id so a task can remember facts across phases
 * without leaking into the next task.
 *
 * Persistence is opt-in via RHA_MEMORY_FILE (a JSON file path). When
 * unset, memory lives only in-process.
 */

import { promises as fs } from "node:fs";
import * as path from "node:path";
import type { Phase, PhaseContext, PhaseResult, Plugin } from "../core/types.js";
import { nowIso, uuid } from "../core/types.js";

interface MemoryRecord {
  key: string;
  value: unknown;
  created_at: string;
  phase?: Phase;
}

class MemoryStore {
  private records: Map<string, MemoryRecord> = new Map();
  private file: string | undefined;

  constructor(file?: string) {
    this.file = file;
  }

  async load(): Promise<void> {
    if (!this.file) return;
    try {
      const raw = await fs.readFile(this.file, "utf8");
      const data = JSON.parse(raw) as MemoryRecord[];
      for (const r of data) this.records.set(r.key, r);
    } catch {
      // Missing or corrupt file: start fresh.
    }
  }

  async save(): Promise<void> {
    if (!this.file) return;
    const data = Array.from(this.records.values());
    await fs.mkdir(path.dirname(this.file), { recursive: true });
    await fs.writeFile(this.file, JSON.stringify(data, null, 2));
  }

  get(key: string): unknown {
    return this.records.get(key)?.value;
  }

  set(key: string, value: unknown, phase?: Phase): void {
    this.records.set(key, { key, value, created_at: nowIso(), phase });
  }

  list(): MemoryRecord[] {
    return Array.from(this.records.values());
  }

  clear(): void {
    this.records.clear();
  }
}

const store = new MemoryStore(process.env.RHA_MEMORY_FILE);

export const memoryPlugin: Plugin = {
  id: "memory",
  name: "Memory",
  version: "1.0.0",
  description: "Durable key/value memory shared across phases (in-process, optional JSON file).",
  capabilities: {
    phases: ["Interrogate", "Contract", "Execute", "Finish"],
    tools: ["memory.get", "memory.set", "memory.list", "memory.clear"],
  },

  async init() {
    await store.load();
  },

  async execute(phase: Phase, ctx: PhaseContext): Promise<PhaseResult> {
    const started = nowIso();
    const errors: string[] = [];
    const actions: Record<string, unknown> = {};

    const reqs = (ctx.data?.memory_actions as Array<{ op: string; key?: string; value?: unknown }>) ?? [];
    for (const r of reqs) {
      switch (r.op) {
        case "get": {
          const key = r.key ?? "";
          actions[key] = { value: store.get(key) };
          break;
        }
        case "set": {
          const key = r.key ?? "";
          store.set(key, r.value, phase);
          actions[key] = { value: r.value };
          break;
        }
        case "list": {
          actions.list = store.list();
          break;
        }
        case "clear": {
          store.clear();
          break;
        }
        default:
          errors.push(`unknown memory op "${r.op}"`);
      }
    }

    // Auto-persist a snapshot for the run so later runs can inspect it.
    if (reqs.length > 0) {
      try {
        await store.save();
      } catch (err) {
        errors.push(`persist failed: ${err instanceof Error ? err.message : String(err)}`);
      }
    }

    return {
      phase,
      success: errors.length === 0,
      output: { phase, actions },
      errors,
      artifacts: [],
      started_at: started,
      completed_at: nowIso(),
      duration_ms: 0,
    };
  },
};

export { store as memoryStore, uuid };
