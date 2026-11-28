// Última release e entregas sem release de todos os projetos (#355), para os
// cards da Sala de controle (#358): uma chamada para a tela inteira. O hub
// não tem push próprio para isso; a leitura sai no mesmo ciclo do sync do
// GitHub que avisa `hub.control_room.changed`, então relê junto dele. Hub
// sem o método (anterior ao 1.28) ou fora do ar = `null`.
import { useCallback, useEffect, useState } from "react";
import type { ProjectRelease } from "../protocol/hub-gen";
import { useTower } from "../state/tower";

export function useHubReleases(): ProjectRelease[] | null {
  const { api, hub } = useTower();
  const [releases, setReleases] = useState<ProjectRelease[] | null>(null);

  const load = useCallback(async () => {
    try {
      setReleases(await api.hubCall<ProjectRelease[]>("hub.projects.release"));
    } catch {
      setReleases(null);
    }
  }, [api]);

  useEffect(() => {
    if (!hub.connected) {
      setReleases(null);
      return;
    }
    void load();
    return api.onHubPush((e) => {
      if (e.event === "hub.control_room.changed") void load();
    });
  }, [api, hub.connected, load]);

  return releases;
}
