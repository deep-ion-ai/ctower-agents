// Lista de contas na Sala de controle (#107): cada conta uma vez, com as
// duas janelas, o reset e os ambientes que a usam; ambientes de conta
// desconhecida aparecem à parte, sem selo, com a explicação.
import "./plan.css";
import { useState } from "react";
import { SideGroup } from "../shell/SidePanel";
import { useTower } from "../state/tower";
import type { StatuslineInstall } from "../protocol/gen";
import type { PlanEnvRef } from "./data";
import { accountLabel, statuslineIssue, UNKNOWN_ACCOUNT_TEXT } from "./format";
import { PlanBadge } from "./PlanBadge";
import { usePlan } from "./usePlan";

const INSTALLED: Record<StatuslineInstall, string> = {
  unchanged: "statusline já estava em dia",
  installed: "statusline instalado",
  updated: "statusline atualizado",
  chained: "statusline instalado (o anterior roda dentro dele)",
};

/** Ambiente da lista: nome e, se for o caso, por que não há leitura (com o botão de instalar no desktop). */
function EnvLine({ e, onInstalled }: { e: PlanEnvRef; onInstalled(): void }) {
  const { api, web, status } = useTower();
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ text: string; warn: boolean } | null>(null);
  const issue = statuslineIssue(e.statusline);
  // Instalar só no desktop e no ambiente conectado a esta UI (`env.statusline.install`).
  const canInstall = !!issue && !web && !!status[e.env]?.connected;
  const install = async () => {
    setBusy(true);
    setMsg(null);
    try {
      const r = await api.call<StatuslineInstall>(e.env, "env.statusline.install");
      setMsg({ text: INSTALLED[r] ?? "statusline instalado", warn: false });
      onInstalled();
    } catch (err) {
      setMsg({ text: `falhou: ${err instanceof Error ? err.message : String(err)}`, warn: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <li className="plan-env">
      <span className="cr-sub">{e.name}</span>
      {issue && (
        <span className="cr-sub" data-warn="true">
          {issue}
        </span>
      )}
      {canInstall && (
        <button
          type="button"
          className="btn btn-sm plan-install"
          onClick={install}
          disabled={busy}
          aria-busy={busy}
          aria-label={`Instalar o statusline em ${e.name}`}
          title="Registra o statusline do Control Tower no Claude Code do ambiente (ctower statusline install)"
        >
          {busy ? "Instalando…" : "Instalar statusline"}
        </button>
      )}
      {msg && (
        <span className="cr-sub" role="status" data-warn={msg.warn ? "true" : undefined}>
          {msg.text}
        </span>
      )}
    </li>
  );
}

/** Grupo "Plano das contas" do painel lateral da Sala de controle. */
export function PlanAccounts({ now }: { now: number }) {
  const { data, reload } = usePlan(now);
  const reloadSoon = () => void reload();
  if (!data.accounts.length && !data.unknown.length) return null;
  return (
    <SideGroup title="Plano das contas" count={data.accounts.length}>
      {data.accounts.map((a) => {
        const label = accountLabel(a);
        return (
          <li key={a.key} className="sp-row cr-item plan-account" aria-label={label}>
            <span className="plan-account-name">
              {label}
              {a.account.subscription_type && <span className="cr-sub">{a.account.subscription_type}</span>}
            </span>
            <PlanBadge account={a} now={now} variant="lines" />
            <ul className="plan-envs" aria-label={`Ambientes da conta ${label}`}>
              {a.envs.map((e) => (
                <EnvLine key={e.env} e={e} onInstalled={reloadSoon} />
              ))}
            </ul>
          </li>
        );
      })}
      {data.unknown.length > 0 && (
        <li className="sp-row cr-item plan-account" aria-label="Conta desconhecida">
          <span className="plan-account-name">Conta desconhecida</span>
          <span className="cr-sub">{UNKNOWN_ACCOUNT_TEXT}</span>
          <ul className="plan-envs" aria-label="Ambientes de conta desconhecida">
            {data.unknown.map((e) => (
              <EnvLine key={e.env} e={e} onInstalled={reloadSoon} />
            ))}
          </ul>
        </li>
      )}
    </SideGroup>
  );
}
