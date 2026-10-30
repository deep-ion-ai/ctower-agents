import { useState } from "react";
import { useTower } from "../../state/tower";
import { errText } from "../../hub/calls";

/**
 * O que as seções de configuração recebem de quem as hospeda: `run` roda uma
 * mudança no hub e relê tudo (`true` = deu certo); `busy` trava as ações
 * enquanto uma roda. O erro e a mensagem de sucesso ficam com o hospedeiro.
 */
export interface SettingsRun {
  busy: boolean;
  /**
   * `f` devolve a mensagem de sucesso. Se falhar, `handle` pode tratar o erro
   * (devolvendo `true`, como num pedido de `confirm`); senão ele vira o erro.
   */
  run: (f: () => Promise<string | null>, handle?: (message: string) => boolean) => Promise<boolean>;
  /** Mostra um erro que a própria seção detectou, antes de chamar o hub. */
  fail: (message: string) => void;
}

export function useSettingsRun(): SettingsRun & { error: string | null; done: string | null } {
  const { refreshHub } = useTower();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const run: SettingsRun["run"] = async (f, handle) => {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      setDone(await f());
      await refreshHub();
      return true;
    } catch (e) {
      const m = errText(e);
      if (!handle?.(m)) setError(m);
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, run, error, done, fail: setError };
}
