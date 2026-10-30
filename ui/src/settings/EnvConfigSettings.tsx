import { useCallback, useEffect, useState } from "react";
import type { ConfigItem, Phase } from "../protocol/gen";
import { useTower } from "../state/tower";

// Chaves do config.toml editáveis pela UI (#365, métodos do #328):
// `env.config.get` lista o valor em uso e o padrão; `env.config.set` grava e
// aplica na hora, sem restart. Como no resto das Configurações, o valor vale
// ao sair do campo (ou com Enter), sem botão de salvar.

export type EnvConfigKey = "update_ref" | "gh_sync_interval_secs";

/** Menor intervalo de sync aceito pelo daemon, além de 0 (desligado). */
export const MIN_GH_SYNC_SECS = 30;

/** Mesma regra do daemon (`valid_ref` em env_config.rs): nome de branch ou tag. */
export function validRef(r: string): boolean {
  return (
    r.length > 0 &&
    r.length <= 200 &&
    !/^[-/.]/.test(r) &&
    !/[/.]$/.test(r) &&
    !r.endsWith(".lock") &&
    !r.includes("..") &&
    !r.includes("//") &&
    !r.includes("@{") &&
    !/[\s\p{Cc}~^:?*[\\]/u.test(r)
  );
}

/** `gh_sync_interval_secs`: inteiro, 0 ou pelo menos 30. */
export function validSyncInterval(n: number): boolean {
  return Number.isInteger(n) && (n === 0 || n >= MIN_GH_SYNC_SECS);
}

export interface EnvConfig {
  /** `null` = carregando, sem conexão ou daemon sem `env.config.*` (ver `error`). */
  items: Partial<Record<EnvConfigKey, ConfigItem>> | null;
  /** Por que não dá para editar (daemon antigo ou edição desativada). */
  error: string | null;
  /** Erro da última gravação de cada chave (mensagem do daemon). */
  setErrors: Partial<Record<EnvConfigKey, string>>;
  set(key: EnvConfigKey, value: string | number): Promise<void>;
}

/** Valores de `env.config.get` do ambiente, relidos ao (re)conectar. */
export function useEnvConfig(env: string | undefined): EnvConfig {
  const { api, status, refresh } = useTower();
  const on = !!env && !!status[env]?.connected;
  const [items, setItems] = useState<EnvConfig["items"]>(null);
  const [error, setError] = useState<string | null>(null);
  const [setErrors, setSetErrors] = useState<EnvConfig["setErrors"]>({});

  useEffect(() => {
    setItems(null);
    setError(null);
    setSetErrors({});
    if (!env || !on) return;
    let alive = true;
    api.call<ConfigItem[]>(env, "env.config.get").then(
      (list) => alive && setItems(Object.fromEntries(list.map((i) => [i.key, i]))),
      (e) => alive && setError(String(e)),
    );
    return () => {
      alive = false;
    };
  }, [api, env, on]);

  const set = useCallback(
    async (key: EnvConfigKey, value: string | number) => {
      if (!env) return;
      try {
        const item = await api.call<ConfigItem>(env, "env.config.set", { key, value });
        setItems((all) => ({ ...all, [key]: item }));
        setSetErrors(({ [key]: _, ...rest }) => rest);
        // Canal novo muda o `update` do env.info (o disponível é do ref antigo).
        if (key === "update_ref") void refresh(env);
      } catch (e) {
        setSetErrors((all) => ({ ...all, [key]: String(e) }));
      }
    },
    [api, env, refresh],
  );

  return { items, error, setErrors, set };
}

/** Item diferente do padrão (marcador de alterada). */
export const configChanged = (i: ConfigItem | undefined) => !!i && String(i.value) !== String(i.default);

/** Campo do canal (`update_ref`). */
export function UpdateRefControl({ cfg }: { cfg: EnvConfig }) {
  const item = cfg.items?.update_ref;
  const current = item ? String(item.value) : "";
  const [ref, setRef] = useState(current);
  useEffect(() => setRef(current), [current]);
  const valid = validRef(ref.trim());
  const commit = () => {
    if (valid && ref.trim() !== current) void cfg.set("update_ref", ref.trim());
    else if (!valid) setRef(current);
  };
  return (
    <div className="set-inline">
      <input
        aria-label="Canal de atualização"
        className="set-input set-select mono"
        value={ref}
        aria-invalid={!valid}
        onChange={(e) => setRef(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
      {!valid && <span className="state-off hint">Nome de branch ou tag (sem espaços, ~ ^ : ? * [ \).</span>}
      {cfg.setErrors.update_ref && (
        <span role="alert" className="state-off hint">
          {cfg.setErrors.update_ref}
        </span>
      )}
    </div>
  );
}

/** Campo do intervalo do sync com o GitHub (`gh_sync_interval_secs`). */
export function SyncIntervalControl({ cfg }: { cfg: EnvConfig }) {
  const item = cfg.items?.gh_sync_interval_secs;
  const current = item ? String(item.value) : "";
  const [secs, setSecs] = useState(current);
  useEffect(() => setSecs(current), [current]);
  const n = Number(secs);
  const valid = secs.trim() !== "" && validSyncInterval(n);
  const commit = () => {
    if (valid && secs !== current) void cfg.set("gh_sync_interval_secs", n);
    else if (!valid) setSecs(current);
  };
  return (
    <>
      <input
        aria-label="Intervalo de sincronização (segundos)"
        className="set-input-num"
        type="number"
        min={0}
        value={secs}
        aria-invalid={!valid}
        onChange={(e) => setSecs(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
      {!valid && <span className="state-off hint">0 (desligado) ou pelo menos {MIN_GH_SYNC_SECS} segundos.</span>}
      {valid && n === 0 && <span className="muted hint">desligado</span>}
      {cfg.setErrors.gh_sync_interval_secs && (
        <span role="alert" className="state-off hint">
          {cfg.setErrors.gh_sync_interval_secs}
        </span>
      )}
    </>
  );
}

const PHASE: Partial<Record<Phase, string>> = {
  done: "concluído",
  failed: "falhou",
  rolled_back: "falhou e voltou à versão de antes",
};

/**
 * Reverter à versão anterior do ctowerd (`env.rollback`): mesma confirmação
 * do Atualizar da Manutenção, com as sessões vivas que o restart encerra
 * (`force` só quando há alguma, como no `env.update`).
 */
export function RollbackControl({ env }: { env: string }) {
  const { api, status, infos, dashboards } = useTower();
  const on = !!status[env]?.connected;
  const u = infos[env]?.update;
  const live = dashboards[env]?.live_sessions.length ?? 0;
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [startedAfter, setStartedAfter] = useState<number | null>(null);

  // Estado do rollback disparado aqui (ou o último, se foi um rollback).
  const s = u?.state;
  const mine = s && s.kind === "rollback" && (startedAfter === null || s.started_at > startedAfter) ? s : null;
  const running = startedAfter !== null && (!mine || !PHASE[mine.phase]);

  const start = async () => {
    setConfirm(false);
    setBusy(true);
    setError(null);
    setStartedAfter(s?.started_at ?? 0);
    try {
      await api.call(env, "env.rollback", { force: live > 0 });
    } catch (e) {
      setError(String(e));
      setStartedAfter(null);
    } finally {
      setBusy(false);
    }
  };

  if (!on) return <span className="muted">sem conexão</span>;
  if (!u) return <span className="muted">daemon sem suporte a atualização</span>;
  return (
    <div className="set-stack">
      <div className="set-inline">
        <button type="button" className="btn" disabled={busy || running || confirm} onClick={() => setConfirm(true)}>
          Reverter
        </button>
        {running ? (
          <span role="status" className="muted">
            revertendo… o daemon reinicia e a conexão volta sozinha
          </span>
        ) : (
          mine && (
            <span role="status" className={mine.phase === "done" ? "muted" : "state-off"}>
              último rollback: {PHASE[mine.phase] ?? mine.phase}
              {mine.error ? `: ${mine.error}` : ""}
            </span>
          )
        )}
      </div>
      {confirm && (
        <div role="group" aria-label="Confirmar reversão" className="confirm-box">
          <p>
            Voltar o ctowerd à versão anterior à última atualização? O daemon reinicia e a conexão cai por alguns
            segundos.
          </p>
          {live > 0 && (
            <p>
              <strong>{live} sessão(ões) viva(s)</strong> serão encerradas pelo restart e voltarão como interrompidas na
              Recuperação.
            </p>
          )}
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => setConfirm(false)}>
              Cancelar
            </button>
            <button type="button" className={live > 0 ? "btn btn-danger" : "btn btn-primary"} onClick={start}>
              {live > 0 ? "Reverter e encerrar sessões" : "Reverter"}
            </button>
          </div>
        </div>
      )}
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
    </div>
  );
}
