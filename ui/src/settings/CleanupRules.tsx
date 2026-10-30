import { useEffect, useState } from "react";
import type { CleanupConfig, CleanupStatus } from "../protocol/gen";
import { useTower } from "../state/tower";

/** Chaves de `[cleanup]` no `config.toml`, na ordem mostrada. */
export const CLEANUP_KEYS = [
  "check_interval_hours",
  "auto",
  "min_age_hours",
  "min_idle_hours",
  "end_idle_sessions_hours",
  "remove_agent_transcripts",
  "target_idle_days",
] as const satisfies readonly (keyof CleanupConfig)[];

type Key = (typeof CLEANUP_KEYS)[number];

const onOff = (b: boolean) => (b ? "ligado" : "desligado");
const orOff = (n: number, unit: string) => (n === 0 ? "desligado" : `${n} ${unit}`);

/** Valor de cada chave, como o ambiente o aplica, e para que serve. */
const RULES: Record<Key, { value: (c: CleanupConfig) => string; what: string }> = {
  check_interval_hours: {
    value: (c) => orOff(c.check_interval_hours, "h"),
    what: "Intervalo do job que monta o plano (roda também no boot); 0 desliga o job.",
  },
  auto: {
    value: (c) => onOff(c.auto),
    what: "Depois de cada verificação, executa as etapas pendentes. Desligado, só monta o plano.",
  },
  min_age_hours: {
    value: (c) => `${c.min_age_hours} h`,
    what: "Horas desde a conclusão antes de limpar o work item.",
  },
  min_idle_hours: {
    value: (c) => `${c.min_idle_hours} h`,
    what: "Horas desde o último evento de sessão do work item antes de limpá-lo.",
  },
  end_idle_sessions_hours: {
    value: (c) => orOff(c.end_idle_sessions_hours, "h"),
    what: "Encerra a sessão viva e ociosa de work item concluído; desligado, só à mão.",
  },
  remove_agent_transcripts: {
    value: (c) => onOff(c.remove_agent_transcripts),
    what: "Apaga os transcripts do Claude Code junto com o worktree, só com backup.",
  },
  target_idle_days: {
    value: (c) => orOff(c.target_idle_days, "dia(s)"),
    what: "Dias sem atividade antes de apagar só o target/ de worktree que fica; 0 desliga.",
  },
};

/** Estado das regras na UI: `null` = carregando; `false` = daemon sem suporte. */
type Rules = CleanupConfig | false | null;

/**
 * Ajustes da limpeza do ambiente (#337): as chaves de `[cleanup]` com o
 * valor que o daemon aplica (`cleanup.status`). Ver o plano, verificar e
 * limpar ficam em Manutenção › Limpeza.
 */
export function CleanupRules({ env }: { env: string }) {
  const { status, api } = useTower();
  const on = !!status[env]?.connected;
  const [rules, setRules] = useState<Rules>(null);

  useEffect(() => {
    setRules(null);
    if (!on) return;
    let alive = true;
    api.call<CleanupStatus>(env, "cleanup.status").then(
      (st) => alive && setRules(st.config),
      () => alive && setRules(false),
    );
    return () => {
      alive = false;
    };
  }, [api, env, on]);

  useEffect(
    () =>
      api.onPush((e) => {
        if (e.event === "cleanup.status" && e.env === env) setRules((e.data as CleanupStatus).config);
      }),
    [api, env],
  );

  const value = (k: Key) =>
    !on ? "sem conexão" : rules === false ? "daemon sem suporte a limpeza" : rules ? RULES[k].value(rules) : "—";

  return (
    <div className="table-wrap">
      <table aria-label="Regras de limpeza">
        <thead>
          <tr>
            <th>Chave</th>
            <th>Valor</th>
            <th>Para quê</th>
          </tr>
        </thead>
        <tbody>
          {CLEANUP_KEYS.map((k) => (
            <tr key={k}>
              <td className="mono">{k}</td>
              <td>{value(k)}</td>
              <td className="muted">{RULES[k].what}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
