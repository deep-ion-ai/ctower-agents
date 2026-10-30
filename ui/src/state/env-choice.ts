// Ambiente de uma tela por ambiente (Configurações, Manutenção): o do
// endereço; sem ele, o primeiro conectado, fixado na primeira escolha.
import { useEffect, useState } from "react";
import { useTower } from "./tower";

/**
 * Ambiente mostrado pela tela. Sem ambiente no endereço (ou com um que não
 * existe mais): o primeiro conectado, fixado na primeira escolha (cair a
 * conexão não troca o ambiente na tela); sem nenhum conectado, o primeiro.
 */
export function useEnvChoice(addressEnv: string | undefined): string | undefined {
  const { envs, status } = useTower();
  const ids = envs.map((e) => e.id).join(",");
  const known = (id: string | undefined) => (id && envs.some((e) => e.id === id) ? id : undefined);
  const auto = envs.find((e) => status[e.id]?.connected)?.id ?? envs[0]?.id;
  const [sticky, setSticky] = useState<string | undefined>(undefined);
  const statusKnown = Object.keys(status).length > 0;
  useEffect(() => {
    if (!(sticky && ids.split(",").includes(sticky)) && statusKnown && auto) setSticky(auto);
  }, [sticky, ids, statusKnown, auto]);
  return known(addressEnv) ?? known(sticky) ?? auto;
}
