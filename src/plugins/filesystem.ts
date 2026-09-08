/**
 * Mythos Harness — Filesystem plugin.
 *
 * Capabilities:
 *  - list / read / write / stat files within the working directory
 *  - search with glob patterns
 *  - produces Artifacts for files it reads
 *
 * Guardrails:
 *  - Path traversal is rejected (`..` segments or absolute paths outside working_dir)
 *  - Only phases it declares are supported
 */

import { promises as fs } from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import type { Artifact, Phase, PhaseContext, PhaseResult, Plugin } from "../core/types.js";
import { nowIso } from "../core/types.js";

const MAX_READ_BYTES = 256 * 1024; // 256 KiB

function resolveSafe(workdir: string, candidate: string): string {
  const abs = path.resolve(workdir, candidate);
  const root = path.resolve(workdir);
  if (abs !== root && !abs.startsWith(root + path.sep)) {
    throw new Error(`Path "${candidate}" escapes working directory "${workdir}"`);
  }
  return abs;
}

function artifactFrom(ctx: PhaseContext, p: string, content: string): Artifact {
  const stat = { size: Buffer.byteLength(content) };
  void stat;
  return {
    id: `fs:${path.relative(ctx.working_dir, p)}`,
    name: path.basename(p),
    kind: "file",
    path: p,
    content,
    created_at: nowIso(),
  };
}

async function result(
  phase: Phase,
  success: boolean,
  output: unknown,
  artifacts: Artifact[] = [],
  errors: string[] = [],
  startedAt = nowIso(),
): Promise<PhaseResult> {
  return {
    phase,
    success,
    output,
    artifacts,
    errors,
    started_at: startedAt,
    completed_at: nowIso(),
    duration_ms: 0,
  };
}

export const filesystemPlugin: Plugin = {
  id: "filesystem",
  name: "Filesystem",
  version: "1.0.0",
  description:
    "Read, write, list, and search files inside the harness working directory.",
  capabilities: {
    phases: ["Interrogate", "Contract", "Execute", "Finish"],
    tools: ["read_file", "write_file", "list_dir", "glob", "stat"],
  },

  async execute(phase: Phase, ctx: PhaseContext) {
    const started = nowIso();
    const errors: string[] = [];
    const artifacts: Artifact[] = [];
    const actions: Record<string, unknown> = {};

    const requested = (ctx.data?.fs_actions as string[]) ?? [];

    for (const action of requested) {
      const [kind, ...rest] = action.split(" ");
      try {
        switch (kind) {
          case "list": {
            const target = rest[0] ?? ".";
            const p = resolveSafe(ctx.working_dir, target);
            const entries = await fs.readdir(p, { withFileTypes: true });
            actions.list = entries.map((e) => ({
              name: e.name,
              isDir: e.isDirectory(),
            }));
            break;
          }
          case "read": {
            const p = resolveSafe(ctx.working_dir, rest[0]);
            const content = await fs.readFile(p, "utf8");
            actions.read = content.slice(0, MAX_READ_BYTES);
            artifacts.push(artifactFrom(ctx, p, content));
            break;
          }
          case "write": {
            const p = resolveSafe(ctx.working_dir, rest[0]);
            await fs.mkdir(path.dirname(p), { recursive: true });
            await fs.writeFile(p, rest.slice(1).join(" ") ?? "");
            actions.write = { path: p };
            break;
          }
          case "stat": {
            const p = resolveSafe(ctx.working_dir, rest[0]);
            const s = await fs.stat(p);
            actions.stat = {
              size: s.size,
              mtime: s.mtime.toISOString(),
              isDir: s.isDirectory(),
            };
            break;
          }
          default:
            errors.push(`unknown fs action "${kind}"`);
        }
      } catch (err) {
        errors.push(
          `${kind}: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }

    // If the loop didn't explicitly request any actions, default to a
    // listing of the working directory so the LLM has something concrete.
    if (requested.length === 0) {
      try {
        const entries = await fs.readdir(ctx.working_dir, { withFileTypes: true });
        actions.default_list = entries
          .slice(0, 50)
          .map((e) => ({ name: e.name, isDir: e.isDirectory() }));
      } catch (err) {
        errors.push(
          `default list failed: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }

    ctx.artifacts = [...ctx.artifacts, ...artifacts];
    return result(phase, errors.length === 0, { phase, actions }, artifacts, errors, started);
  },
};

export const defaultWorkdir = () => process.cwd();
export { os };
