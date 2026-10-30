import { useCallback, useEffect, useRef, useState } from "react";
import { useTower } from "../../state/tower";
import type { SecretsList } from "../../protocol/hub-gen";
import { errText } from "../../hub/calls";

export interface ProjectSecrets {
  list: SecretsList | null;
  error: string | null;
  /** Relê a lista (ao fechar o editor dos secrets do projeto, #8). */
  reload: () => Promise<void>;
}

/** `hub.secrets.list`, relido junto com a visão do hub (a entrega muda com os pulls). */
export function useSecrets(enabled = true): ProjectSecrets {
  const { api, hub } = useTower();
  const [list, setList] = useState<SecretsList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);
  const load = useCallback(async () => {
    // Web: `hub.secrets.list` fica fora do /web/ws (devolve valores de variáveis).
    if (!enabled) return;
    const n = ++seq.current;
    try {
      const l = await api.hubCall<SecretsList>("hub.secrets.list");
      if (n !== seq.current) return;
      setList(l);
      setError(null);
    } catch (e) {
      if (n === seq.current) setError(errText(e));
    }
  }, [api, enabled]);
  useEffect(() => {
    void load();
  }, [load, hub.envs]);
  return { list, error, reload: load };
}

/** Secrets e variáveis próprios do projeto e a entrega deles por ambiente. */
export function projectSecretsOf(secrets: ProjectSecrets, owner: string, repo: string) {
  const scope = `${owner}/${repo}`.toLowerCase();
  return {
    own: (secrets.list?.items ?? []).filter((i) => typeof i.scope === "object" && i.scope.project === scope),
    delivery: (secrets.list?.delivery ?? []).filter((d) => d.projects.includes(scope)),
  };
}
