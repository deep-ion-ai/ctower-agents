// Hub › Dispositivos (issue #203, spec do épico #205 §7.2): os aparelhos
// inscritos na Web Push (`hub.push.list`), os eventos de cada um
// (`hub.push.update`), testar (`hub.push.test`) e remover (`hub.push.remove`).
// Na PWA, também "Este dispositivo": ativar as notificações (pedido de
// permissão a partir do toque) e, no iOS fora da tela de início, a
// instrução para instalar. No desktop a mesma lista aparece, sem inscrever.
import { useCallback, useEffect, useState } from "react";
import type { HubPushDevice, PushEvent, PushRemoved, PushTestResult } from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import { ago } from "../util/time";
import { errText } from "./calls";
import { PUSH_EVENTS, disablePush, enablePush, pushSupport, type PushSupport } from "../web/pwa";

function ThisDevice({
  support,
  mine,
  busy,
  onEnable,
  onDisable,
}: {
  support: PushSupport;
  mine: HubPushDevice | undefined;
  busy: boolean;
  onEnable(): void;
  onDisable(id: number): void;
}) {
  switch (support) {
    case "ios-home-screen":
      return (
        <div className="confirm-box" role="note" aria-label="Notificações no iPhone e no iPad">
          <p>
            <span className="strong">No iPhone e no iPad, as notificações exigem a PWA na tela de início</span> (iOS
            16.4 ou mais novo). No Safari, toque em <span className="strong">Compartilhar</span> ›{" "}
            <span className="strong">Adicionar à Tela de Início</span>, abra o Control Tower pelo ícone e volte aqui para
            ativar.
          </p>
        </div>
      );
    case "unsupported":
      return <p className="muted">Este navegador não recebe Web Push.</p>;
    case "denied":
      return (
        <p className="muted">
          As notificações estão bloqueadas para este site: libere nas configurações do navegador e recarregue.
        </p>
      );
    case "ok":
      return mine && mine.disabled_at === null ? (
        <div className="form-actions">
          <span className="state-on">Este dispositivo recebe notificações.</span>
          <button type="button" className="btn" disabled={busy} onClick={() => onDisable(mine.id)}>
            Desativar neste dispositivo
          </button>
        </div>
      ) : (
        <div className="form-actions">
          <button type="button" className="btn btn-primary" disabled={busy} onClick={onEnable}>
            {mine ? "Reativar notificações neste dispositivo" : "Ativar notificações neste dispositivo"}
          </button>
        </div>
      );
  }
}

export function HubDevices({ now }: { now: number }) {
  const { api, web } = useTower();
  const [list, setList] = useState<HubPushDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [support] = useState<PushSupport | null>(() => (web ? pushSupport() : null));

  const load = useCallback(async () => {
    try {
      setList(await api.hubCall<HubPushDevice[]>("hub.push.list"));
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  }, [api]);

  useEffect(() => {
    void load();
  }, [load]);

  const run = async (f: () => Promise<string | null>) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      setDone(await f());
      await load();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  // Sem `await` antes do enablePush: o pedido de permissão precisa sair do toque.
  const enable = () => void run(() => enablePush(api).then(() => "Notificações ativadas neste dispositivo."));
  const disable = (id: number) =>
    void run(() => disablePush(api, id).then(() => "Notificações desativadas neste dispositivo."));

  const toggle = (d: HubPushDevice, ev: PushEvent) => {
    const events = d.events.includes(ev) ? d.events.filter((e) => e !== ev) : [...d.events, ev];
    void run(async () => {
      await api.hubCall<HubPushDevice>("hub.push.update", { id: d.id, events });
      return null;
    });
  };

  const test = (d: HubPushDevice) =>
    void run(async () => {
      const r = await api.hubCall<PushTestResult>("hub.push.test", { id: d.id });
      if (r.delivered) return "Notificação de teste enviada.";
      throw new Error(r.error ?? "o serviço de push recusou a notificação");
    });

  const remove = (d: HubPushDevice) =>
    void run(async () => {
      const r = await api.hubCall<PushRemoved>("hub.push.remove", { id: d.id });
      return r.removed ? "Dispositivo removido." : "O dispositivo já não estava inscrito.";
    });

  const mine = list?.find((d) => d.current);
  const name = (d: HubPushDevice) => d.label ?? d.service;

  return (
    <section aria-labelledby="hub-dispositivos" className="panel settings-section">
      <h2 id="hub-dispositivos">Dispositivos · {list?.length ?? "…"}</h2>
      <p className="muted hint">
        Aparelhos que recebem as notificações da PWA (Web Push). Sessões com Remote Control já avisam pelo app Claude e
        não geram &quot;Aguardando você&quot; aqui. Revogar a sessão web de um aparelho também remove a inscrição dele.
      </p>
      {web && support ? (
        <div className="hub-devices-this">
          <h3>Este dispositivo</h3>
          <ThisDevice support={support} mine={mine} busy={busy} onEnable={enable} onDisable={disable} />
        </div>
      ) : (
        <p className="muted">
          Os aparelhos se inscrevem pela PWA, em Hub › Dispositivos no celular. Aqui dá para escolher os eventos, testar
          e remover.
        </p>
      )}
      {!list ? (
        !error && <p className="muted">Carregando…</p>
      ) : list.length === 0 ? (
        <p className="muted">Nenhum dispositivo inscrito.</p>
      ) : (
        <ul className="hub-devices" aria-label="Dispositivos inscritos">
          {list.map((d) => (
            <li key={d.id} className="hub-device">
              <div className="hub-device-head">
                <span className="strong">{name(d)}</span>
                {d.current && (
                  <span className="badge" data-tone="running">
                    este dispositivo
                  </span>
                )}
                {d.disabled_at !== null && (
                  <span className="badge" data-tone="error">
                    desativado após {d.failures} falhas
                  </span>
                )}
              </div>
              <p className="muted">
                {d.service} · inscrito {ago(d.created_at, now)}
                {d.last_ok_at !== null ? ` · última entrega ${ago(d.last_ok_at, now)}` : " · nenhuma entrega ainda"}
              </p>
              <fieldset className="hub-device-events" disabled={busy}>
                <legend className="sr-only">Eventos de {name(d)}</legend>
                {PUSH_EVENTS.map((e) => (
                  <label key={e.event} title={e.hint}>
                    <input type="checkbox" checked={d.events.includes(e.event)} onChange={() => toggle(d, e.event)} />{" "}
                    {e.label}
                  </label>
                ))}
              </fieldset>
              <div className="form-actions">
                <button type="button" className="btn btn-sm" disabled={busy} onClick={() => test(d)}>
                  Testar<span className="sr-only"> {name(d)}</span>
                </button>
                <button type="button" className="btn btn-sm" disabled={busy} onClick={() => remove(d)}>
                  Remover<span className="sr-only"> {name(d)}</span>
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
      {done && (
        <p role="status" className="state-on">
          {done}
        </p>
      )}
    </section>
  );
}
