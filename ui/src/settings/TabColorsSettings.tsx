import { useEffect, useState } from "react";
import type { TabColors } from "../api/types";
import {
  MIN_CONTRAST,
  TAB_KIND_LABEL,
  TAB_KINDS,
  THEME_TAB_DEFAULTS,
  colorOf,
  emptyTabColors,
  hasTabColors,
  kindContrast,
  normalizeHex,
  withTabColor,
  type TabKind,
} from "../shell/tabColors";
import { resolveTheme, type Theme } from "../state/theme";
import { useTower } from "../state/tower";

// Cores das abas por tipo e por tema (#73), em Configurações › Aparência.
// Cada campo tem o seletor nativo, o HEX (vale ao sair do campo ou com
// Enter; inválido mostra o erro e mantém a última cor válida) e o Padrão.

const THEME_LABEL: Record<Theme, string> = { dark: "Tema escuro", light: "Tema claro" };
const FIELD_LABEL = { fg: "Texto", bg: "Fundo" } as const;

/** Cores das abas diferentes do padrão (marcador de alterada). */
export const tabColorsChanged = (tabs: TabColors | undefined) => hasTabColors(tabs);

export function TabColorsControl() {
  const { config, setConfig } = useTower();
  // Começa no tema em uso: é o que a pessoa está vendo.
  const [theme, setTheme] = useState<Theme>(() => resolveTheme(config.appearance.theme));
  const tabs = config.appearance.tabs;
  const save = (next: TabColors) => void setConfig({ ...config, appearance: { ...config.appearance, tabs: next } });

  return (
    <div className="set-stack tabc">
      <div className="set-inline">
        <div role="radiogroup" aria-label="Tema das cores das abas" className="tabc-themes">
          {(["dark", "light"] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="radio"
              aria-checked={theme === t}
              className="btn btn-sm"
              data-on={theme === t ? "true" : undefined}
              onClick={() => setTheme(t)}
            >
              {THEME_LABEL[t]}
            </button>
          ))}
        </div>
        <button
          type="button"
          className="btn btn-sm"
          disabled={!hasTabColors(tabs, theme)}
          onClick={() => save({ ...emptyTabColors(), ...tabs, [theme]: emptyTabColors()[theme] })}
        >
          Restaurar padrão<span className="sr-only"> das cores das abas do {THEME_LABEL[theme].toLowerCase()}</span>
        </button>
      </div>
      {TAB_KINDS.map((kind) => (
        <KindRow
          key={`${theme}-${kind}`}
          kind={kind}
          theme={theme}
          tabs={tabs}
          onChange={(k, v) => save(withTabColor(tabs, theme, kind, k, v))}
        />
      ))}
    </div>
  );
}

function KindRow({
  kind,
  theme,
  tabs,
  onChange,
}: {
  kind: TabKind;
  theme: Theme;
  tabs: TabColors | undefined;
  onChange: (k: "fg" | "bg", v: string | null) => void;
}) {
  const ratio = kindContrast(tabs, theme, kind);
  const low = ratio < MIN_CONTRAST;
  const label = TAB_KIND_LABEL[kind];
  return (
    <fieldset className="tabc-kind" data-kind={kind}>
      <legend>{label}</legend>
      {(["fg", "bg"] as const).map((k) => (
        <ColorField
          key={k}
          id={`tabc-${theme}-${kind}-${k}`}
          label={`${FIELD_LABEL[k]} de ${label}`}
          name={FIELD_LABEL[k]}
          value={colorOf(tabs?.[theme]?.[kind], k)}
          fallback={THEME_TAB_DEFAULTS[theme][k]}
          onChange={(v) => onChange(k, v)}
        />
      ))}
      <span
        className={low ? "state-off hint tabc-ratio" : "muted hint tabc-ratio"}
        aria-label={`Contraste de ${label}`}
        role={low ? "status" : undefined}
      >
        Contraste {ratio.toFixed(1)}:1{low && ` — abaixo de ${MIN_CONTRAST}:1, pode ficar difícil de ler`}
      </span>
    </fieldset>
  );
}

function ColorField({
  id,
  label,
  name,
  value,
  fallback,
  onChange,
}: {
  id: string;
  label: string;
  name: string;
  /** Cor gravada (`null` = cor do tema). */
  value: string | null;
  /** Cor do tema, mostrada no seletor quando o campo está vazio. */
  fallback: string;
  onChange: (v: string | null) => void;
}) {
  const [text, setText] = useState(value ?? "");
  const [error, setError] = useState(false);
  useEffect(() => {
    setText(value ?? "");
    setError(false);
  }, [value]);

  const commit = () => {
    if (text.trim() === "") {
      setError(false);
      if (value !== null) onChange(null);
      return;
    }
    const hex = normalizeHex(text);
    if (!hex) {
      setError(true);
      return;
    }
    setError(false);
    setText(hex);
    if (hex !== value) onChange(hex);
  };

  return (
    <div className="tabc-field">
      <span className="tabc-name">{name}</span>
      <input
        type="color"
        aria-label={`${label} (seletor)`}
        value={value ?? fallback}
        onChange={(e) => onChange(normalizeHex(e.target.value))}
      />
      <input
        id={id}
        aria-label={`${label} (HEX)`}
        className="set-input mono tabc-hex"
        placeholder="padrão"
        spellCheck={false}
        value={text}
        aria-invalid={error}
        aria-describedby={error ? `${id}-err` : undefined}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
      <button
        type="button"
        className="link-btn"
        aria-label={`Padrão: ${label}`}
        disabled={value === null && text === ""}
        onClick={() => {
          setText("");
          setError(false);
          onChange(null);
        }}
      >
        Padrão
      </button>
      {error && (
        <span id={`${id}-err`} role="alert" className="state-off hint">
          HEX inválido: use #RGB ou #RRGGBB.
        </span>
      )}
    </div>
  );
}
