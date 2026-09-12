import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Ignored local worktrees may contain test copies. Keep them outside this
    // checkout's test graph so a path-filtered run remains local.
    exclude: [
      "**/node_modules/**",
      "**/dist/**",
      "**/cypress/**",
      "**/.{idea,git,cache,output,temp}/**",
      "**/.claude/**",
    ],
  },
});
