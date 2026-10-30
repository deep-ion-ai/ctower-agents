import { useCallback, useEffect, useState, type ReactNode } from "react";
import type { TokenStatus } from "../protocol/gen";
import { useTower } from "../state/tower";

const SOURCE: Record<string, string> = {
  token: "token configurado no Control Tower",
  gh: "login do gh na distro (gh auth token)",
  none: "nenhum: configure um token ou rode gh auth login na distro",
};

/** Chamada a quem guarda o token (`github.status|set_token|clear_token`). */
export type TokenCall = <T>(method: string, params?: object) => Promise<T>;

/** Explica de onde vem e onde fica o token do GitHub do ambiente. */
export const ENV_TOKEN_HINT = (
  <>
    O token precisa ler issues e pull requests dos repositórios (e o conteúdo, para clonar repositórios privados). Ele é
    validado no GitHub e gravado na distro em <span className="mono">~/.config/ctower/github.token</span> com permissão
    0600; o app não guarda nem mostra o token. Sem token configurado, o daemon usa o login do{" "}
    <span className="mono">gh</span>.
  </>
);

/**
 * Token do GitHub do daemon de um ambiente (spec §11): o token vai para o
 * daemon, que o valida e grava na distro (0600). A UI só vê a conta e a
 * origem. O do hub fica no Hub › Token do GitHub (`GithubTokenPanel`).
 */
export function GithubSettings({ env }: { env: string }) {
  const { status, api, refresh } = useTower();
  const on = !!status[env]?.connected;
  const call = useCallback<TokenCall>(
    (method, params) => (on ? api.call(env, method, params) : Promise.reject(new Error("ambiente desconectado"))),
    [api, env, on],
  );

  if (!on) return <p className="muted">Conecte o ambiente para configurar o acesso ao GitHub.</p>;
  return (
    <GithubToken
      id={`gh-${env}`}
      call={call}
      // Token novo: sincroniza já, para a árvore refletir o acesso.
      onSaved={() => void api.call(env, "sync.now").then(() => refresh(env), () => {})}
    />
  );
}

/** Conta, origem e troca do token do GitHub de um daemon (ambiente ou hub). */
export function GithubTokenPanel({
  id,
  title,
  call,
  onSaved,
  hint,
}: {
  id: string;
  title: string;
  call: TokenCall;
  onSaved?: () => void;
  hint: ReactNode;
}) {
  return (
    <section aria-labelledby={id} className="panel settings-section">
      <h2 id={id}>{title}</h2>
      <GithubToken id={id} call={call} onSaved={onSaved} />
      <p className="muted hint">{hint}</p>
    </section>
  );
}

/** Conta, origem, troca e remoção do token (corpo do painel). */
export function GithubToken({ id, call, onSaved }: { id: string; call: TokenCall; onSaved?: () => void }) {
  const [st, setSt] = useState<TokenStatus | null>(null);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setSt(await call<TokenStatus>("github.status"));
    } catch (e) {
      setError(String(e));
    }
  }, [call]);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (method: "github.set_token" | "github.clear_token") => {
    setBusy(true);
    setError(null);
    try {
      setSt(await call<TokenStatus>(method, method === "github.set_token" ? { token: token.trim() } : {}));
      setToken("");
      onSaved?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="set-stack">
      <div className="env-row">
        <div className="fact">
          <span className="label">Conta</span>
          <span className={st?.login ? "strong" : "state-off"}>
            {st ? (st.login ? `@${st.login}` : "sem acesso") : "verificando…"}
          </span>
        </div>
        <div className="fact">
          <span className="label">Origem do token</span>
          <span>{st ? SOURCE[st.source] ?? st.source : "—"}</span>
        </div>
        {st?.error && (
          <div className="fact">
            <span className="label">Erro</span>
            <span className="state-off">{st.error}</span>
          </div>
        )}
      </div>
      <form
        className="token-form"
        aria-label="Token do GitHub"
        onSubmit={(e) => {
          e.preventDefault();
          void act("github.set_token");
        }}
      >
        <div className="field">
          <label htmlFor={`${id}-token`}>Novo token (personal access token)</label>
          <input
            id={`${id}-token`}
            type="password"
            autoComplete="off"
            value={token}
            placeholder="ghp_… ou github_pat_…"
            onChange={(e) => setToken(e.target.value)}
          />
        </div>
        <div className="form-actions">
          {st?.source === "token" && (
            <button type="button" className="btn" disabled={busy} onClick={() => act("github.clear_token")}>
              Remover token
            </button>
          )}
          <button type="submit" className="btn btn-primary" disabled={busy || !token.trim()}>
            {busy ? "Validando…" : "Salvar token"}
          </button>
        </div>
      </form>
      {error && (
        <p role="alert" className="start-error">
          {error}
        </p>
      )}
    </div>
  );
}
