// Renderizador do console: WebGL quando der; senão (ou se o contexto cair),
// o renderizador DOM padrão do xterm, sem perder o buffer.

export type Renderer = "webgl" | "dom";

/** O que usamos do addon WebGL (um fake nos testes). */
export interface WebglLike {
  onContextLoss(cb: () => void): { dispose(): void };
  dispose(): void;
}

/**
 * Carrega o WebGL no terminal já aberto. Sem suporte (o construtor ou o
 * `loadAddon` lançam), fica no DOM; na perda do contexto, descartar o addon
 * devolve o terminal ao DOM.
 */
export function loadWebgl<W extends WebglLike>(
  load: (addon: W) => void,
  make: () => W,
  onChange: (r: Renderer) => void = () => {},
): Renderer {
  let addon: W;
  try {
    addon = make();
    load(addon);
  } catch (e) {
    console.warn("WebGL indisponível; o console usa o renderizador DOM", e);
    return "dom";
  }
  const sub = addon.onContextLoss(() => {
    sub.dispose();
    addon.dispose();
    onChange("dom");
  });
  return "webgl";
}
