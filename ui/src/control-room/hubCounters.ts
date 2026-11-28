// Contadores do topo da Sala de controle que vêm do hub (#357): execuções de
// épico ativas (`hub.epic_run.list`) e o piloto automático
// (`hub.autopilot.status`). Relidos nos pushes de cada um; hub sem o método
// (anterior ao recurso) ou fora do ar = `null`, e o chip some.
import { useCallback, useEffect, useState } from "react";
import type { AutopilotStatus, EpicRun } from "../protocol/hub-gen";
import { useTower } from "../state/tower";

export interface PilotCounter {
  paused: boolean;
  /** Projetos com o modo autônomo ligado. */
  projects: number;
}

export function useHubCounters(): { epics: number | null; pilot: PilotCounter | null } {
  const { api, hub } = useTower();
  const [epics, setEpics] = useState<number | null>(null);
  const [pilot, setPilot] = useState<PilotCounter | null>(null);

  const loadEpics = useCallback(async () => {
    try {
      setEpics((await api.hubCall<EpicRun[]>("hub.epic_run.list", {})).length);
    } catch {
      setEpics(null);
    }
  }, [api]);
  const loadPilot = useCallback(async () => {
    try {
      const s = await api.hubCall<AutopilotStatus>("hub.autopilot.status");
      setPilot({ paused: s.settings.paused, projects: s.projects.length });
    } catch {
      setPilot(null);
    }
  }, [api]);

  useEffect(() => {
    if (!hub.connected) {
      setEpics(null);
      setPilot(null);
      return;
    }
    void loadEpics();
    void loadPilot();
    return api.onHubPush((e) => {
      // `hub.epic_run.changed` não vai para a web; `hub.control_room.changed`
      // sai junto dele e vai para os dois.
      if (e.event === "hub.epic_run.changed" || e.event === "hub.control_room.changed") void loadEpics();
      if (e.event === "hub.autopilot.changed") void loadPilot();
    });
  }, [api, hub.connected, loadEpics, loadPilot]);

  return { epics, pilot };
}
