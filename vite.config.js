import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  root: "src",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    rolldownOptions: {
      output: {
        codeSplitting: {
          groups: [
            {
              name: "vendor",
              test: /node_modules/,
              minSize: 20000,
              maxSize: 350000,
            },
          ],
        },
      },
    },
  },
  server: {
    strictPort: true,
  },
});
