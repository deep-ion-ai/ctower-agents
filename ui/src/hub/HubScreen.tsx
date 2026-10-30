import "../settings/settings.css";
import "./hub.css";
import { useCallback, useState, type ReactNode } from "react";
import { useNow } from "../App";
import { useNav, type Screen } from "../state/nav";
import { useTower } from "../state/tower";
import { hubUpdateText } from "../maintenance/Updates";
import { ago } from "../util/time";
import { GithubTokenPanel, type TokenCall } from "../settings/GithubSettings";
import type { QuarantineDiscarded, QuarantineGroup } from "../protocol/hub-gen";
import { SideContent, SideGroup, SidePanel, SideRow } from "../shell/SidePanel";
import { useShell } from "../shell/Shell";
import { Icon } from "../shell/icons";
import { shortcutLabel } from "../shell/layout";
import { HubEnvs } from "./HubEnvs";
import { HealthPanel, useHubHealth } from "./HubHealth";
import { ProposalsPanel, useHubProposals } from "./HubProposals";
import { HubSecrets } from "./HubSecrets";
import { HubRecovery } from "./HubRecovery";
import { HubOverview } from "./HubOverview";
import { HubWebSessions } from "./HubWebSessions";
import { HubDevices } from "./HubDevices";
import { hubSectionOnWeb } from "../web/mode";
import { errText } from "./calls";
import { settingsTab } from "../settings/sections";
import { splitProjectKey } from "../util/hub";
import {
  HUB_GROUPS,
  HUB_SECTION_LABEL,
  hubCounts,
  hubGroupOf,
  hubTab,
  type HubSection,
  type SectionCount,
} from "./sections";

/**
 * Tela do hub (`cthubd`, spec do hub como plano de controle §8), em seções
 * (#313): o painel lateral lista as seções por grupo (Operação, Cadastro,
 * Segurança) com os contadores e, no rodapé, versão, protocolo,
 * revisão da configuração e último snapshot. Cada seção abre como aba de
 * editor (`hubTab`): a Visão geral reúne o que pede ação; as outras são os
 * painéis de sempre (propostas e saúde do gestor, quarentena de push,
 * ambientes ligados, secrets e variáveis, chave de recuperação e token do
 * GitHub do hub). Projetos e atribuições saiu na #18: cada projeto se
 * configura na aba Configurações dele, e o cadastro e a ordem ficam no
 * índice de Projetos. Backup e snapshots do hub ficam em
 * Manutenção (#338). Terminal, sessões e recuperação continuam em cada
 * ambiente.
 */
export function HubScreen({ screen }: { screen: Extract<Screen, { name: "hub" }> }) {
  const { hub, api, refreshHub, web } = useTower();
  const nav = useNav();
  const now = useNow();
  const section = screen.section ?? "overview";
  const hidden = web && !hubSectionOnWeb(section);
  const call = useCallback<TokenCall>((method, params) => api.hubCall(method, params), [api]);
  const { health, error: healthError } = useHubHealth();
  const proposals = useHubProposals();
  const open = useCallback((s: HubSection) => nav.openTab(hubTab(s)), [nav]);

  const counts = hub.connected
    ? hubCounts({
        warnings: hub.health?.warnings ?? [],
        quarantine: hub.health?.quarantine ?? [],
        pending: proposals.pending.length,
        health,
        envs: hub.envs.length,
      })
    : {};

  let body: ReactNode;
  if (hidden) {
    // Endereço antigo ou link de fora: o grupo Segurança não existe na web.
    body = (
      <div className="hub-empty">
        <h2>{HUB_SECTION_LABEL[section]}</h2>
        <p role="status" className="muted">
          Esta seção fica só no app desktop: secrets, chave de recuperação e token do GitHub não passam pelo acesso web.
        </p>
      </div>
    );
  } else if (!hub.conn) {
    body = (
      <div className="hub-empty">
        <h2>Hub não configurado</h2>
        <p className="muted">
          A conexão com o hub não está configurada. Projetos, atribuições e a visão consolidada ficam no hub (
          <span className="mono">cthubd</span>); as sessões continuam em cada ambiente.
        </p>
        <div className="form-actions">
          <button type="button" className="btn btn-primary" onClick={() => nav.openTab(settingsTab({ section: "connections" }))}>
            Configurar o hub
          </button>
        </div>
      </div>
    );
  } else if (!hub.connected) {
    body = (
      <div className="hub-empty">
        <h2>Hub indisponível</h2>
        <p role="status" className="muted">
          Hub indisponível: cadastro, atribuições e a visão consolidada voltam quando ele responder. As sessões nos
          ambientes seguem normalmente, com a última configuração recebida.
        </p>
        <p className="mono muted small">indisponível · {hub.status?.error ?? "tentando de novo"}</p>
      </div>
    );
  } else {
    body = (
      <Section
        section={section}
        now={now}
        open={open}
        proposals={proposals}
        health={health}
        healthError={healthError}
        onRegister={(key) =>
          // O cadastro abre no índice de Projetos (#18). Chave com mais de uma barra é caminho do GitLab
          // (`grupo/projeto/app`, #352): o formulário abre em Jira + GitLab, sem URL (o host não vem na quarentena).
          nav.openTab(
            hubTab("projects", {
              register: key.split("/").length > 2 ? { url: "", kind: "jira" } : { url: `https://github.com/${key}` },
            }),
          )
        }
        call={call}
        onTokenSaved={() => void refreshHub()}
      />
    );
  }

  return (
    <main className="hub-screen" aria-labelledby="hub-h1">
      <SideContent>
        <HubSide section={section} counts={counts} now={now} />
      </SideContent>
      <nav className="hub-crumbs" aria-label="Trilha">
        <ol>
          <li>Hub</li>
          <li>{hubGroupOf(section)}</li>
          <li aria-current="page">{HUB_SECTION_LABEL[section]}</li>
        </ol>
      </nav>
      {/* A Visão geral mostra o título; nas outras seções o painel já tem o seu. */}
      {(section !== "overview" || !hub.connected) && (
        <h1 id="hub-h1" className="sr-only">
          Hub · {HUB_SECTION_LABEL[section]}
        </h1>
      )}
      <div className="settings hub-content">
        <div className="settings-main">{body}</div>
      </div>
    </main>
  );
}

function Section({
  section,
  now,
  open,
  proposals,
  health,
  healthError,
  onRegister,
  call,
  onTokenSaved,
}: {
  section: HubSection;
  now: number;
  open(s: HubSection): void;
  proposals: ReturnType<typeof useHubProposals>;
  health: ReturnType<typeof useHubHealth>["health"];
  healthError: string | null;
  onRegister(key: string): void;
  call: TokenCall;
  onTokenSaved(): void;
}) {
  switch (section) {
    case "overview":
      return <HubOverview now={now} proposals={proposals} health={health} healthError={healthError} open={open} />;
    case "proposals":
      return <ProposalsPanel now={now} data={proposals} />;
    case "health":
      return <HealthPanel health={health} error={healthError} />;
    case "quarantine":
      return <Quarantine now={now} onRegister={onRegister} />;
    case "projects":
      // Nunca chega aqui: `screenTab` (state/nav.tsx) leva o endereço ao projeto ou ao índice (#18).
      return null;
    case "envs":
      return <HubEnvs now={now} />;
    case "web-sessions":
      return <HubWebSessions now={now} />;
    case "devices":
      return <HubDevices now={now} />;
    case "secrets":
      return <HubSecrets />;
    case "recovery":
      return <HubRecovery />;
    case "github":
      return (
        <GithubTokenPanel
          id="hub-gh"
          title="GitHub do hub"
          call={call}
          onSaved={onTokenSaved}
          hint={
            <>
              O hub valida os repositórios no cadastro e sincroniza com o GitHub todos os projetos, com o token dele (
              <span className="mono">~/.config/cthub/github.token</span> na máquina do hub, ou o login do{" "}
              <span className="mono">gh</span> de lá). Cada ambiente mantém o próprio token para clonar.
            </>
          }
        />
      );
  }
}

/** Painel lateral do Hub: nome e conexão, seções por grupo com contador e, no rodapé, os dados do hub. */
function HubSide({
  section,
  counts,
  now,
}: {
  section: HubSection;
  counts: Partial<Record<HubSection, SectionCount>>;
  now: number;
}) {
  const { hub, web } = useTower();
  const nav = useNav();
  const shell = useShell();
  const info = hub.info;
  const groups = web
    ? HUB_GROUPS.map((g) => ({ ...g, sections: g.sections.filter(hubSectionOnWeb) })).filter(
        (g) => g.sections.length > 0,
      )
    : HUB_GROUPS;
  const backup = info?.backup ?? hub.health?.backup;
  const state = !hub.conn ? "não configurado" : hub.connected ? "conectado" : "indisponível";
  const title = hub.conn ? `Hub · ${info?.name ?? hub.conn.url}` : "Hub";

  return (
    <SidePanel
      title={title}
      actions={
        <>
          <span className="hub-conn" data-on={hub.connected ? "true" : undefined}>
            {state}
          </span>
          <button
            type="button"
            className="ibtn"
            aria-label={`Recolher painel lateral (${shortcutLabel("B")})`}
            onClick={shell.toggleSide}
          >
            <Icon name="sidebar" size={15} />
          </button>
        </>
      }
      footer={
        info && (
          <dl className="hub-facts" aria-label="Dados do hub">
            <Fact label="Versão" value={`${info.version}${info.commit ? ` · ${info.commit.slice(0, 7)}` : ""}`} mono />
            <Fact label="Protocolo" value={info.protocol} mono />
            {hub.update && <Fact label="Atualização" value={hubUpdateText(hub.update)} />}
            <Fact label="Revisão da configuração" value={String(info.config_rev)} mono />
            {backup && (
              <Fact
                label="Último snapshot"
                value={
                  backup.interval_hours === 0
                    ? "snapshots desligados"
                    : backup.newest_at
                      ? `${ago(backup.newest_at, now)} em ${backup.dir}`
                      : `nenhum ainda em ${backup.dir}`
                }
              />
            )}
          </dl>
        )
      }
    >
      {groups.map((g) => (
        <SideGroup key={g.label} title={g.label}>
          {g.sections.map((s) => {
            const c = counts[s];
            return (
              <SideRow
                key={s}
                label={HUB_SECTION_LABEL[s]}
                selected={s === section}
                description={c?.text}
                trailing={
                  c && (
                    <span className="hub-count" data-tone={c.tone}>
                      {c.n}
                    </span>
                  )
                }
                onClick={() => nav.openTab(hubTab(s))}
              />
            );
          })}
        </SideGroup>
      ))}
    </SidePanel>
  );
}

function Fact({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="fact">
      <dt className="label">{label}</dt>
      <dd className={mono ? "mono" : undefined}>{value}</dd>
    </div>
  );
}

/** Linhas de push de projeto não cadastrado (`hub.quarantine.list|discard`). */
function Quarantine({ now, onRegister }: { now: number; onRegister: (key: string) => void }) {
  const { hub, api, refreshHub, web } = useTower();
  const [confirm, setConfirm] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ text: string; error: boolean } | null>(null);
  const groups = hub.health?.quarantine ?? [];
  const byProject = new Map<string, QuarantineGroup[]>();
  for (const g of groups) byProject.set(g.project_key, [...(byProject.get(g.project_key) ?? []), g]);

  const discard = async (key: string) => {
    // Na última barra: o `owner` do GitLab tem barras (`grupo/projeto`, #352).
    const { owner, repo } = splitProjectKey(key);
    setBusy(true);
    try {
      const r = await api.hubCall<QuarantineDiscarded>("hub.quarantine.discard", { owner, repo, confirm: true });
      setMsg({ text: `${r.discarded} linha(s) de ${key} descartada(s).`, error: false });
      setConfirm(null);
      await refreshHub();
    } catch (e) {
      setMsg({ text: errText(e), error: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="hub-quarentena" className="panel settings-section">
      <h2 id="hub-quarentena">Quarentena</h2>
      {byProject.size === 0 ? (
        <p className="muted">Nada em quarentena.</p>
      ) : (
        [...byProject.entries()].map(([key, gs]) => {
          const rows = gs.reduce((n, g) => n + g.rows, 0);
          const last = Math.max(...gs.map((g) => g.last_received_at));
          return (
            <div key={key} className="env-row" role="group" aria-label={`Quarentena de ${key}`}>
              <QFact label="Projeto" value={key} mono />
              <QFact label="Linhas" value={`${rows} · ${gs.map((g) => `${g.env_name} ${g.rows}`).join(", ")}`} />
              <QFact label="Último envio" value={ago(last, now)} />
              {/* Web: cadastro e descarte ficam no desktop (spec do épico #205 §4.1). */}
              {!web && (
                <div className="env-actions">
                  <button type="button" className="btn" onClick={() => onRegister(key)}>
                    Cadastrar projeto
                  </button>
                  <button type="button" className="btn btn-danger" onClick={() => setConfirm(key)}>
                    Descartar
                  </button>
                </div>
              )}
              {confirm === key && (
                <div role="group" aria-label={`Confirmar descarte de ${key}`} className="confirm-box hub-wide">
                  <p>
                    Descartar {rows} linha(s) de {key}? Não há como recuperá-las depois, exceto pelo{" "}
                    <span className="mono">ctower hub resync</span> do ambiente.
                  </p>
                  <div className="form-actions">
                    <button type="button" className="btn" onClick={() => setConfirm(null)}>
                      Cancelar
                    </button>
                    <button type="button" className="btn btn-danger" disabled={busy} onClick={() => discard(key)}>
                      Descartar linhas
                    </button>
                  </div>
                </div>
              )}
            </div>
          );
        })
      )}
      {msg && (
        <p role={msg.error ? "alert" : "status"} className={msg.error ? "start-error" : "state-on"}>
          {msg.text}
        </p>
      )}
      <p className="muted hint">
        O hub nunca cria projeto a partir do que um ambiente envia: o que chega de um projeto não cadastrado fica aqui e é
        reaplicado ao cadastrar o projeto.
      </p>
    </section>
  );
}

function QFact({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="fact">
      <span className="label">{label}</span>
      <span className={mono ? "mono" : undefined}>{value}</span>
    </div>
  );
}
