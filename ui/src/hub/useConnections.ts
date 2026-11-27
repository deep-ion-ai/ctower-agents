import { useCallback, useEffect, useState } from "react";
import type { Connection } from "../protocol/gen";
import { useTower } from "../state/tower";
import { errText } from "./calls";

/**
 * Conexões do hub (`hub.connections.list`) para os formulários de projeto e
 * a tela de Conexões. Só no desktop: a escrita e a leitura das conexões não
 * estão no escopo da web (`hub.connections.*` é do app), então a web recebe
 * `null` e mostra os projetos sem o nome da conexão.
 */
export function useHubConnections(): {
  connections: Connection[] | null;
  error: string | null;
  reload: () => Promise<void>;
} {
  const { api, hub, web } = useTower();
  const [connections, setConnections] = useState<Connection[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const on = hub.connected && !web;

  const reload = useCallback(async () => {
    try {
      setConnections(await api.hubCall<Connection[]>("hub.connections.list"));
      setError(null);
    } catch (e) {
      setError(errText(e));
    }
  }, [api]);

  useEffect(() => {
    if (on) void reload();
    else setConnections(null);
  }, [on, reload]);

  return { connections, error, reload };
}
