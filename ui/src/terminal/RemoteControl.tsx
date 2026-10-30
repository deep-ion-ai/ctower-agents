import qrcode from "qrcode-generator";
import { useEffect, useMemo, useRef, useState } from "react";
import type { DaemonApi } from "../api/daemon";

/** Margem (em módulos) em volta do código, para a câmera achar as bordas. */
const QUIET = 4;

/**
 * QR code de `value` em SVG (qrcode-generator, sem dependências). Módulos
 * escuros sobre fundo claro nos dois temas (tokens do terminal, que não
 * mudam com o tema), como a câmera espera.
 */
export function QrCode({ value, label, size = 176 }: { value: string; label: string; size?: number }) {
  const { d, n } = useMemo(() => {
    const qr = qrcode(0, "M");
    qr.addData(value);
    qr.make();
    const count = qr.getModuleCount();
    let path = "";
    for (let r = 0; r < count; r++)
      for (let c = 0; c < count; c++) if (qr.isDark(r, c)) path += `M${c + QUIET} ${r + QUIET}h1v1h-1z`;
    return { d: path, n: count + 2 * QUIET };
  }, [value]);
  return (
    <svg className="rc-qr" role="img" aria-label={label} width={size} height={size} viewBox={`0 0 ${n} ${n}`} shapeRendering="crispEdges">
      <rect className="rc-qr-bg" width={n} height={n} />
      <path className="rc-qr-fg" d={d} />
    </svg>
  );
}

/**
 * Link da sessão no Remote Control com o QR code para abrir no celular,
 * "Abrir no navegador" e "Copiar link" (#197). Esc ou clicar fora fecha.
 */
export function RemotePanel({
  url,
  title,
  api,
  onClose,
}: {
  url: string;
  title: string;
  api: Pick<DaemonApi, "openUrl" | "writeClipboard">;
  onClose: () => void;
}) {
  const ref = useRef<HTMLElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    ref.current?.querySelector("button")?.focus();
    const away = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) close.current();
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, []);
  const run = (f: () => Promise<void>) =>
    f().catch((e) => {
      setError(String(e));
    });
  return (
    <section
      ref={ref}
      className="rc-panel"
      role="dialog"
      aria-label="Remote Control"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="rc-panel-head">
        <span className="sb-dot" data-on="true" aria-hidden="true" />
        <strong>Remote Control conectado</strong>
        <button type="button" className="ibtn rc-close" aria-label="Fechar" onClick={onClose}>
          ×
        </button>
      </div>
      <p className="muted small rc-panel-title">{title}</p>
      <QrCode value={url} label={`QR code do link ${url}`} />
      <p className="muted small">Aponte a câmera do celular para continuar a sessão no app Claude.</p>
      <span className="mono rc-link">{url}</span>
      <div className="rc-panel-actions">
        <button type="button" className="btn btn-sm" onClick={() => void run(() => api.openUrl(url))}>
          Abrir no navegador
        </button>
        <button
          type="button"
          className="btn btn-sm"
          onClick={() =>
            void run(async () => {
              await api.writeClipboard(url);
              setCopied(true);
            })
          }
        >
          {copied ? "Link copiado" : "Copiar link"}
        </button>
      </div>
      {error && (
        <p role="alert" className="session-error">
          {error}
        </p>
      )}
    </section>
  );
}
