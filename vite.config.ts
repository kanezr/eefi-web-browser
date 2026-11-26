import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [
    svelte(),
    {
      name: "stage3-live-verification-fixtures",
      configureServer(server) {
        const fixtures: Record<string, [string, string]> = {
          "fixture.html": ["text/html; charset=utf-8", "fixture.html"],
          "frame.html": ["text/html; charset=utf-8", "frame.html"],
          "worker.js": ["text/javascript; charset=utf-8", "worker.js"],
          "blocked-fetch.txt": ["text/plain; charset=utf-8", "blocked-fetch.txt"],
          "blocked-image.svg": ["image/svg+xml", "blocked-image.svg"],
          "blocked-document.html": ["text/html; charset=utf-8", "blocked-document.html"],
        };
        server.middlewares.use("/__stage3__/", (request, response, next) => {
          const filename = request.url?.split("?")[0]?.split("/").pop();
          const fixture = filename ? fixtures[filename] : undefined;
          if (!fixture) {
            next();
            return;
          }
          void readFile(resolve(process.cwd(), "tests", "stage3-live", fixture[1]))
            .then((contents) => {
              response.setHeader("Content-Type", fixture[0]);
              response.setHeader("Cache-Control", "no-store");
              response.end(contents);
            })
            .catch((error: unknown) => {
              next(error instanceof Error ? error : new Error(String(error)));
            });
        });
      },
    },
  ],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
});
