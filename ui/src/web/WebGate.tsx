// Entrada da PWA (épico #205, spec §3.1): `GET /web/me` decide entre a tela
// de login e o shell. O token de UI do hub é digitado uma vez, vai no corpo
// do `POST /web/login` e não fica em lugar nenhum (nem no estado depois do
// envio). Sessão revogada ou vencida (fechamento 4401 do `/web/ws`) volta
// para o login.
import "./web.css";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import type { WebAuth, WebMe } from "../api/web";

/** Sessão web deste navegador, para o botão de sair e Configurações › Acesso web. */
export interface WebSessionInfo {
  identity: string | null;
  expiresAt: number | null;
  logout(): Promise<void>;
}

const Ctx = createContext<WebSessionInfo | null>(null);

/** `null` fora da web (app desktop) e nos testes de uma tela sozinha. */
export function useWebSession(): WebSessionInfo | null {
  return useContext(Ctx);
}

type GateState =
  | { kind: "checking" }
  | { kind: "error"; message: string }
  | { kind: "login"; notice: string | null }
  | { kind: "in"; me: WebMe };

export const ENDED_NOTICE = "Sua sessão web terminou (foi revogada ou venceu). Entre de novo com o token de UI do hub.";
export const LEFT_NOTICE = "Você saiu. A sessão web deste navegador foi revogada.";

export function WebGate({ auth, children }: { auth: WebAuth; children: ReactNode }) {
  const [state, setState] = useState<GateState>({ kind: "checking" });
  const leaving = useRef(false);

  const check = useCallback(async () => {
    setState({ kind: "checking" });
    try {
      const me = await auth.me();
      if (me.authenticated) {
        auth.start();
        setState({ kind: "in", me });
      } else setState({ kind: "login", notice: null });
    } catch (e) {
      setState({ kind: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }, [auth]);

  useEffect(() => {
    void check();
  }, [check]);

  useEffect(
    () =>
      auth.onLoggedOut(() => {
        const left = leaving.current;
        leaving.current = false;
        setState({ kind: "login", notice: left ? LEFT_NOTICE : ENDED_NOTICE });
      }),
    [auth],
  );

  const logout = useCallback(async () => {
    leaving.current = true;
    try {
      await auth.logout();
    } catch (e) {
      leaving.current = false;
      throw e;
    }
  }, [auth]);

  switch (state.kind) {
    case "checking":
      return (
        <div className="web-gate" role="status">
          <p className="muted">Conferindo a sessão web…</p>
        </div>
      );
    case "error":
      return (
        <div className="web-gate">
          <div className="web-card">
            <h1 className="web-title">Control Tower</h1>
            <p role="alert" className="web-error">
              {state.message}
            </p>
            <button type="button" className="btn" onClick={() => void check()}>
              Tentar de novo
            </button>
          </div>
        </div>
      );
    case "login":
      return <LoginScreen auth={auth} notice={state.notice} onDone={() => void check()} />;
    case "in":
      return (
        <Ctx.Provider value={{ identity: state.me.identity, expiresAt: state.me.expires_at, logout }}>
          {children}
        </Ctx.Provider>
      );
  }
}

/** Tela de login: o token de UI do hub (`~/.config/cthub/ui.token`). */
export function LoginScreen({ auth, notice, onDone }: { auth: WebAuth; notice: string | null; onDone(): void }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const t = token.trim();
    if (!t) return;
    setBusy(true);
    setError(null);
    try {
      await auth.login(t);
      onDone();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    } finally {
      // O token não fica no estado depois do envio, com sucesso ou não.
      setToken("");
    }
  };

  return (
    <div className="web-gate">
      <form className="web-card" aria-labelledby="web-login-h" onSubmit={(e) => void submit(e)}>
        <h1 id="web-login-h" className="web-title">
          Control Tower
        </h1>
        <p className="muted">
          Entre com o token de UI do hub (<span className="mono">~/.config/cthub/ui.token</span> na máquina do hub). Ele
          vai só neste envio: o navegador guarda um cookie de sessão, nunca o token.
        </p>
        {notice && (
          <p role="status" className="web-notice">
            {notice}
          </p>
        )}
        <div className="field">
          <label htmlFor="web-token">Token de UI do hub</label>
          <input
            id="web-token"
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={token}
            disabled={busy}
            onChange={(e) => setToken(e.target.value)}
          />
        </div>
        {error && (
          <p role="alert" className="web-error">
            {error}
          </p>
        )}
        <button type="submit" className="btn btn-primary" disabled={busy || !token.trim()}>
          {busy ? "Entrando…" : "Entrar"}
        </button>
      </form>
    </div>
  );
}
