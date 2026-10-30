import { useEffect, useState } from "react";
import { DEFAULT_PREFS, MAX_FONT, MIN_FONT, effectiveSize, setTermPrefs, useTermPrefs } from "../terminal/prefs";

// Fonte do console nas Configurações (#315). Fica nesta máquina (depende das
// fontes instaladas). Como no VS Code, vale ao sair do campo (ou com Enter),
// sem botão de salvar; o tamanho novo zera o zoom.

/** Fonte e tamanho diferentes do padrão (marcador de alterada). */
export const termFontChanged = (p: { fontFamily: string }) => p.fontFamily !== DEFAULT_PREFS.fontFamily;
export const termSizeChanged = (p: { fontSize: number }) => p.fontSize !== DEFAULT_PREFS.fontSize;

export const restoreTermFont = () => setTermPrefs({ fontFamily: DEFAULT_PREFS.fontFamily });
export const restoreTermSize = () => setTermPrefs({ fontSize: DEFAULT_PREFS.fontSize, zoom: 0 });
export const termCloseChanged = (p: { closeOnExit: boolean }) => p.closeOnExit !== DEFAULT_PREFS.closeOnExit;
export const restoreTermClose = () => setTermPrefs({ closeOnExit: DEFAULT_PREFS.closeOnExit });

/** Fechar a aba quando a sessão sai sem erro (#432). */
export function TermCloseControl() {
  const prefs = useTermPrefs();
  return (
    <div className="notif-row">
      <input
        type="checkbox"
        id="term-close-on-exit"
        checked={prefs.closeOnExit}
        onChange={(e) => setTermPrefs({ closeOnExit: e.target.checked })}
      />
      <label htmlFor="term-close-on-exit">
        <span>Fechar a aba quando a sessão sair sem erro</span>
      </label>
    </div>
  );
}

export function TermFontControl() {
  const prefs = useTermPrefs();
  const [family, setFamily] = useState(prefs.fontFamily);
  useEffect(() => setFamily(prefs.fontFamily), [prefs.fontFamily]);
  const commit = () => {
    if (family.trim() !== prefs.fontFamily) setTermPrefs({ fontFamily: family });
  };
  return (
    <input
      id="term-family"
      aria-label="Fonte"
      className="set-input mono"
      value={family}
      onChange={(e) => setFamily(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && commit()}
    />
  );
}

export function TermSizeControl() {
  const prefs = useTermPrefs();
  const [size, setSize] = useState(String(prefs.fontSize));
  useEffect(() => setSize(String(prefs.fontSize)), [prefs.fontSize]);
  const n = Number(size);
  const valid = Number.isInteger(n) && n >= MIN_FONT && n <= MAX_FONT;
  const commit = () => {
    if (valid && (n !== prefs.fontSize || prefs.zoom !== 0)) setTermPrefs({ fontSize: n, zoom: 0 });
    else if (!valid) setSize(String(prefs.fontSize));
  };
  return (
    <div className="set-inline">
      <input
        id="term-size"
        aria-label="Tamanho da fonte"
        className="set-input set-input-num"
        type="number"
        min={MIN_FONT}
        max={MAX_FONT}
        value={size}
        aria-invalid={!valid}
        onChange={(e) => setSize(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
      {!valid && (
        <span className="state-off hint">
          Entre {MIN_FONT} e {MAX_FONT}.
        </span>
      )}
      {prefs.zoom !== 0 && <span className="muted hint">Zoom atual: {effectiveSize(prefs)} px.</span>}
    </div>
  );
}
