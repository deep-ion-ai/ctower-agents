// Largura da janela nos testes de componente: o jsdom não tem `matchMedia`
// nem layout; aqui ele responde às consultas `(max-width: Npx)` e
// `(min-width: Npx)` pela largura escolhida, e `innerWidth` acompanha.
import { vi } from "vitest";

export function setViewport(width: number) {
  vi.stubGlobal("innerWidth", width);
  vi.stubGlobal("matchMedia", (query: string): MediaQueryList => {
    const max = /\(max-width:\s*(\d+)px\)/.exec(query);
    const min = /\(min-width:\s*(\d+)px\)/.exec(query);
    const matches = (!max || width <= Number(max[1])) && (!min || width >= Number(min[1])) && (!!max || !!min);
    return {
      matches,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    };
  });
}
