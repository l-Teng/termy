import react from "@vitejs/plugin-react";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";
import mdx from "fumadocs-mdx/vite";
import { nitro } from "nitro/vite";

// Default to the `bun` preset for local dev and the Docker image. Deployment
// targets (e.g. Vercel) override this via the NITRO_PRESET env var at build
// time — see vercel.json.
const preset = process.env.NITRO_PRESET ?? "bun";

export default defineConfig({
  server: {
    port: 3000,
  },
  plugins: [
    mdx(),
    tailwindcss(),
    tanstackStart({
      prerender: {
        enabled: true,
      },
      // Prerender the static search index and LLM text dumps so they are
      // served as static files instead of loading the docs source into the
      // server process at runtime.
      pages: [
        { path: '/api/search.json' },
        { path: '/llms.txt' },
        { path: '/llms-full.txt' },
      ],
    }),
    react(),
    nitro({
      preset,
      traceDeps: ["tslib*"],
    }),
  ],
  resolve: {
    tsconfigPaths: true,
    alias: {
      tslib: "tslib/tslib.es6.js",
    },
  },
});
