/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { pwaFiles } from "./pwa/plugin.mjs";

/**
 * Modo `web` (`npm run build:web`, épico #205): a PWA servida pelo cthubd,
 * em `dist-web`. A entrada vira `src/main.web.tsx` e qualquer import de
 * `@tauri-apps/*` quebra o build.
 */
function webMode(): Plugin {
  return {
    name: "ct-web-mode",
    // Antes do resolvedor do Vite, que acharia o pacote e pararia ali.
    enforce: "pre",
    transformIndexHtml: {
      order: "pre",
      handler: (html) =>
        html
          .replace('src="/src/main.tsx"', 'src="/src/main.web.tsx"')
          // PWA instalável (#203): manifest, ícones e o modo standalone no iOS.
          .replace("</head>", `${PWA_HEAD}\n  </head>`),
    },
    resolveId(id, importer) {
      if (id.startsWith("@tauri-apps/")) {
        throw new Error(`modo web não pode importar ${id} (importado por ${importer ?? "?"})`);
      }
      return null;
    },
  };
}

const PWA_HEAD = [
  '<link rel="manifest" href="/manifest.webmanifest" crossorigin="use-credentials" />',
  '<meta name="theme-color" content="#0F1115" />',
  '<link rel="icon" type="image/svg+xml" href="/icons/icon.svg" />',
  '<link rel="apple-touch-icon" href="/icons/apple-touch-icon.png" />',
  '<meta name="mobile-web-app-capable" content="yes" />',
  '<meta name="apple-mobile-web-app-capable" content="yes" />',
  '<meta name="apple-mobile-web-app-title" content="Control Tower" />',
]
  .map((l) => `  ${l}`)
  .join("\n");

// Porta fixa: o Tauri carrega o devUrl em `npm run tauri dev`.
export default defineConfig(({ mode }) => ({
  plugins: mode === "web" ? [react(), webMode(), pwaFiles()] : [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021", outDir: mode === "web" ? "dist-web" : "dist" },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    css: false,
  },
}));
