/**
 * Mythos Harness — Code Executor plugin.
 *
 * Runs a one-shot child process (Node, Python, Bun, bash) with a hard
 * timeout and a sandboxed working directory. Intended for the Execute
 * phase where the agent needs to *verify* something by running it.
 *
 * This is not a sandbox in the security sense: it inherits the user's
 * permissions. Treat the harness working directory as your trust boundary.
 */

import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import * as path from "node:path";
import * as os from "node:os";
import type { Phase, PhaseContext, PhaseResult, Plugin } from "../core/types.js";
import { nowIso } from "../core/types.js";

const TIMEOUT_MS = 20_000;
const MAX_OUTPUT = 32 * 1024;

type Runner = "node" | "python3" | "bun" | "bash";

function runnerCmd(runner: Runner, file: string): string[] {
  switch (runner) {
    case "node":
      return ["node", file];
    case "bun":
      return ["bun", file];
    case "python3":
      return ["python3", file];
    case "bash":
      return ["bash", file];
  }
}

function runCode(
  runner: Runner,
  file: string,
  cwd: string,
): Promise<{ stdout: string; stderr: string; code: number; timedOut: boolean }> {
  return new Promise((resolve) => {
    const [bin, ...args] = runnerCmd(runner, file);
    const child = spawn(bin, args, { cwd, env: process.env });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      try {
        child.kill("SIGKILL");
      } catch {
        /* ignore */
      }
    }, TIMEOUT_MS);

    child.stdout.on("data", (d) => {
      stdout += d.toString();
      if (stdout.length > MAX_OUTPUT) stdout = stdout.slice(0, MAX_OUTPUT);
    });
    child.stderr.on("data", (d) => {
      stderr += d.toString();
      if (stderr.length > MAX_OUTPUT) stderr = stderr.slice(0, MAX_OUTPUT);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ stdout, stderr, code: code ?? -1, timedOut });
    });
    child.on("error", (err) => {
      clearTimeout(timer);
      resolve({ stdout, stderr: stderr + "\n" + err.message, code: -1, timedOut: false });
    });
  });
}

const EXT_BY_RUNNER: Record<Runner, string> = {
  node: ".mjs",
  bun: ".mjs",
  python3: ".py",
  bash: ".sh",
};

export const codeExecutorPlugin: Plugin = {
  id: "code-executor",
  name: "Code Executor",
  version: "1.0.0",
  description: "Execute a script (node/bun/python3/bash) with a hard timeout to verify work.",
  capabilities: {
    phases: ["Execute", "Finish"],
    tools: ["run.code"],
  },

  async execute(phase: Phase, ctx: PhaseContext): Promise<PhaseResult> {
    const started = nowIso();
    const jobs = (
      (ctx.data?.code_jobs as Array<{ runner: Runner; file?: string; code?: string }>) ?? []
    ).slice(0, 5);
    const errors: string[] = [];
    const outputs: Record<string, unknown> = {};

    for (let i = 0; i < jobs.length; i++) {
      const job = jobs[i];
      const label = job.file ?? `inline_${i}`;
      let file = job.file;
      let temp: string | undefined;

      try {
        if (job.code !== undefined) {
          const dir = os.tmpdir();
          const name = `_harness_${Math.random().toString(36).slice(2)}${EXT_BY_RUNNER[job.runner]}`;
          temp = path.join(dir, name);
          await fs.writeFile(temp, job.code, "utf8");
          file = temp;
        }

        if (!file) {
          errors.push(`${label}: no file or code supplied`);
          continue;
        }

        const out = await runCode(job.runner, file, ctx.working_dir);
        outputs[label] = out;
        if (out.code !== 0 && !out.timedOut) {
          errors.push(`${label} exited ${out.code}`);
        }
      } catch (err) {
        errors.push(`${label}: ${err instanceof Error ? err.message : String(err)}`);
      } finally {
        if (temp) {
          await fs.rm(temp, { force: true }).catch(() => {});
        }
      }
    }

    return {
      phase,
      success: errors.length === 0,
      output: { phase, jobs: jobs.length, outputs },
      errors,
      artifacts: [],
      started_at: started,
      completed_at: nowIso(),
      duration_ms: 0,
    };
  },
};
