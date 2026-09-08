/**
 * rHarness — public SDK entry point.
 */

export * from "./types.js";
export * from "./providers.js";
export * from "./localmodel.js";
export { createLoop, runFinishFirstLoop, newContext } from "./loop.js";
export { createHarness, defaultConfig } from "./harness.js";

// Re-export the built-in plugins so users can opt in:
import { filesystemPlugin } from "../plugins/filesystem.js";
import { shellPlugin } from "../plugins/shell.js";
import { memoryPlugin } from "../plugins/memory.js";
import { webSearchPlugin } from "../plugins/websearch.js";
import { codeExecutorPlugin } from "../plugins/codeexecutor.js";
import type { PluginRegistry } from "./types.js";

export const builtinPlugins = [
  filesystemPlugin,
  shellPlugin,
  memoryPlugin,
  webSearchPlugin,
  codeExecutorPlugin,
];

export function registerBuiltinPlugins(registry: PluginRegistry): void {
  for (const p of builtinPlugins) registry.register(p);
}
