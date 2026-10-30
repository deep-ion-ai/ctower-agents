/// <reference types="vite/client" />
// O build web não importa `@tauri-apps/*` (spec do épico #205 §8): percorre
// os imports de valor a partir de src/main.web.tsx, como o bundler faria
// (os `import type` somem na compilação). O `npm run build:web` também
// quebra se aparecer um, pelo plugin do vite.config.

/** Fontes de src/ (chaves "/src/App.tsx"), lidas pelo Vite sem tipos do Node. */
const SOURCES = import.meta.glob<string>(["/src/**/*.ts", "/src/**/*.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
});

/** Especificadores importados como valor (`import x from`, `import "x"`, `export … from`, `import()`). */
export function valueImports(code: string): string[] {
  const out: string[] = [];
  const re =
    /(?:^|\n)\s*(?:import|export)\s+(?!type\s)(?:[^'";]*?\s+from\s+)?["']([^"']+)["']|import\(\s*["']([^"']+)["']\s*\)/g;
  for (const m of code.matchAll(re)) out.push(m[1] ?? m[2]);
  return out;
}

/** Caminho relativo resolvido (posix), com as extensões que o Vite tenta. */
function resolveLocal(from: string, spec: string): string | null {
  const parts = from.split("/").slice(0, -1);
  for (const seg of spec.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg !== ".") parts.push(seg);
  }
  const base = parts.join("/");
  for (const c of [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`]) {
    if (c in SOURCES) return c;
  }
  return null;
}

function graph(entry: string): { files: Set<string>; packages: Set<string> } {
  const files = new Set<string>();
  const packages = new Set<string>();
  const todo = [entry];
  while (todo.length > 0) {
    const f = todo.pop()!;
    if (files.has(f)) continue;
    files.add(f);
    for (const spec of valueImports(SOURCES[f])) {
      if (spec.startsWith(".")) {
        const r = resolveLocal(f, spec);
        // CSS e outros arquivos que não são fonte: não importam pacotes.
        if (r) todo.push(r);
        else if (!/\.(css|svg|png|json)$/.test(spec)) throw new Error(`import não resolvido: ${spec} em ${f}`);
      } else packages.add(spec);
    }
  }
  return { files, packages };
}

test("o leitor de imports ignora `import type`", () => {
  expect(valueImports('import type { A } from "./a";\nimport { b } from "./b";\nimport "./c.css";')).toEqual([
    "./b",
    "./c.css",
  ]);
});

test("o bundle web não importa @tauri-apps/* nem a TauriDaemonApi", () => {
  const { files, packages } = graph("/src/main.web.tsx");
  expect(files.has("/src/api/web.ts")).toBe(true);
  expect(files.has("/src/App.tsx")).toBe(true);
  expect(files.size).toBeGreaterThan(50);
  expect([...packages].filter((p) => p.startsWith("@tauri-apps/"))).toEqual([]);
  expect(files.has("/src/api/daemon.ts")).toBe(false);
});

test("o app desktop importa (controle do teste)", () => {
  const { packages } = graph("/src/main.tsx");
  expect([...packages].some((p) => p.startsWith("@tauri-apps/"))).toBe(true);
});
