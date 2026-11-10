// "Arquivos" do painel da sessão (#167): os `.md` que o agente compartilhou
// com `share_file`. Lidos ao montar (`files.shared`) e de novo a cada push
// `session_files` da mesma sessão; um clique abre no visualizador.
import "./files.css";
import { useEffect, useState } from "react";
import type { SharedFile } from "../protocol/gen";
import { useNav } from "../state/nav";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { basename, fileTab } from "./open";

export const FILES_OLD = "Este ambiente ainda não lista os arquivos compartilhados: atualize o ambiente.";

/** Ambiente anterior ao `files.shared` (-32601, ou a mensagem que atravessa a ponte/hub). */
export const isUnknownMethod = (e: unknown) =>
  /-32601|m[ée]todo (desconhecido|indispon[íi]vel)|method not found/i.test(String(e));

export function SharedFiles({
  env,
  session,
  now,
  heading = "h3",
}: {
  env: string;
  session: string;
  now: number;
  /** Nível do título (o painel do desktop usa h3; a web, h2). */
  heading?: "h2" | "h3";
}) {
  const { api } = useTower();
  const nav = useNav();
  const [files, setFiles] = useState<SharedFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    api.call<SharedFile[]>(env, "files.shared", { session_id: session }).then(
      (f) => {
        if (!live) return;
        setFiles(f);
        setError(null);
      },
      (e) => live && setError(isUnknownMethod(e) ? FILES_OLD : String(e instanceof Error ? e.message : e)),
    );
    return () => {
      live = false;
    };
  }, [api, env, session, tick]);

  useEffect(
    () =>
      api.onPush((e) => {
        if (e.env === env && e.event === "session_files" && e.data?.session_id === session) setTick((t) => t + 1);
      }),
    [api, env, session],
  );

  const H = heading;
  return (
    <section className="sec" aria-label="Arquivos compartilhados">
      <H>
        Arquivos {files && <span className="n">{files.length}</span>}
      </H>
      {error ? (
        <p role={error === FILES_OLD ? "status" : "alert"} className="none">
          {error === FILES_OLD ? FILES_OLD : `Não foi possível listar os arquivos: ${error}`}
        </p>
      ) : !files ? (
        <p className="none" role="status">
          Carregando…
        </p>
      ) : files.length ? (
        <ul className="sf-list">
          {files.map((f) => (
            <li key={f.path}>
              <button
                type="button"
                className="sf-item"
                title={`Abrir ${f.path} no visualizador`}
                onClick={() => nav.openTab(fileTab(env, f.path, { session }, f.title ?? undefined))}
              >
                <span className="sf-name">{f.title || basename(f.path)}</span>
                <span className="sf-path mono">
                  {f.title ? `${basename(f.path)} · ` : ""}
                  {ago(f.shared_at, now)}
                </span>
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="none">Nenhum arquivo compartilhado pelo agente nesta sessão.</p>
      )}
    </section>
  );
}
