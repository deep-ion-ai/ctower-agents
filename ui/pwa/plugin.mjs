// Plugin do Vite para a PWA (issue #203): no `vite build --mode web`, emite
// o manifest, os ícones (ui/pwa/icons, gerados por scripts/pwa-icons.mjs) e
// o sw.js com a lista do shell e a versão do cache. Fica em .mjs (com tipos
// em plugin.d.mts) para o vite.config.ts não precisar dos tipos do Node.
import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const PWA_DIR = dirname(fileURLToPath(import.meta.url));

/**
 * Lista do shell que o service worker guarda: o index.html (como "/"), os
 * arquivos do build (sem mapas e sem as fontes .woff antigas: os navegadores
 * usam .woff2) e os estáticos da PWA. Nunca dados.
 */
export function shellList(bundleFiles, statics) {
  const files = bundleFiles.filter(
    (f) => f !== "index.html" && f !== "sw.js" && !f.endsWith(".map") && !f.endsWith(".woff"),
  );
  return ["/", ...[...files, ...statics].map((f) => `/${f}`)]
    .filter((v, i, a) => a.indexOf(v) === i)
    .sort();
}

/** Versão do cache: muda quando muda algum arquivo com hash do build. */
export function shellVersion(shell) {
  return createHash("sha256").update(shell.join("\n")).digest("hex").slice(0, 12);
}

/** sw.js com a lista e a versão. */
export function buildServiceWorker(shell) {
  const src = readFileSync(join(PWA_DIR, "sw.js"), "utf8");
  const out = src
    .replace('const VERSION = "__CT_VERSION__";', `const VERSION = "${shellVersion(shell)}";`)
    .replace("const SHELL = __CT_SHELL__;", `const SHELL = ${JSON.stringify(shell)};`);
  if (out.includes("__CT_")) throw new Error("pwa/sw.js: marcadores __CT_* não preenchidos");
  return out;
}

export function pwaFiles() {
  return {
    name: "ct-pwa",
    apply: "build",
    generateBundle(_opts, bundle) {
      const statics = ["manifest.webmanifest", ...readdirSync(join(PWA_DIR, "icons")).map((f) => `icons/${f}`)];
      for (const f of statics) {
        this.emitFile({ type: "asset", fileName: f, source: readFileSync(join(PWA_DIR, f)) });
      }
      // theme-boot.js vem do public/ (o Vite copia à parte).
      const shell = shellList([...Object.keys(bundle), "theme-boot.js"], statics);
      this.emitFile({ type: "asset", fileName: "sw.js", source: buildServiceWorker(shell) });
    },
  };
}
