/**
 * Mythos Harness — Shell plugin.
 *
 * Executes shell commands through the platform shell with a hard
 * timeout and working-directory pinning. Output (stdout + stderr) is
 * captured and returned. Long-running processes are not supported.
 */

import { spawn } from "node:child_process";
import type { Phase, PhaseContext, PhaseResult, Plugin } from "../core/types.js";
import { nowIso } from "../core/types.js";

const TIMEOUT_MS = 30_000;
const MAX_OUTPUT_BYTES = 64 * 1024;

function runShell(command: string, cwd: string): Promise<{ stdout: string; stderr: string; code: number; timedOut: boolean }> {
  return new Promise((resolve) => {
    const child = spawn(command, { cwd, shell: true, env: process.env });
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
      if (stdout.length > MAX_OUTPUT_BYTES) {
        stdout = stdout.slice(0, MAX_OUTPUT_BYTES);
        try {
          child.kill("SIGKILL");
        } catch {
          /* ignore */
        }
      }
    });
    child.stderr.on("data", (d) => {
      stderr += d.toString();
      if (stderr.length > MAX_OUTPUT_BYTES) {
        stderr = stderr.slice(0, MAX_OUTPUT_BYTES);
      }
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ stdout, stderr, code: code ?? -1, timedOut });
    });
    child.on("error", (err) => {
      clearTimeout(timer);
      resolve({
        stdout,
        stderr: stderr + "\n" + err.message,
        code: -1,
        timedOut,
      });
    });
  });
}

export const shellPlugin: Plugin = {
  id: "shell",
  name: "Shell",
  version: "1.0.0",
  description: "Execute shell commands with a hard timeout inside the working directory.",
  capabilities: {
    phases: ["Execute"],
    tools: ["run_shell"],
  },

  async execute(phase: Phase, ctx: PhaseContext): Promise<PhaseResult> {
    const started = nowIso();
    const commands = (ctx.data?.shell_commands as string[]) ?? [];
    const errors: string[] = [];
    const outputs: Record<string, unknown> = {};

    for (const cmd of commands) {
      try {
        const out = await runShell(cmd, ctx.working_dir);
        outputs[cmd] = {
          stdout: out.stdout,
          stderr: out.stderr,
          code: out.code,
          timedOut: out.timedOut,
        };
        if (out.code !== 0 && !out.timedOut) {
          errors.push(`command failed (exit ${out.code}): ${cmd}`);
        }
      } catch (err) {
        errors.push(`command error: ${err instanceof Error ? err.message : String(err)}`);
      }
    }

    return {
      phase,
      success: errors.length === 0,
      output: { phase, commands: commands.length, outputs },
      errors,
      artifacts: [],
      started_at: started,
      completed_at: nowIso(),
      duration_ms: 0,
    };
  },
};
