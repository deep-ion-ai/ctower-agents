// Leitura do hub para os itens de "Precisa de você" de #360: PRs com check
// falho (`hub.control_room.projects`), sessões vivas do piloto
// (`hub.autopilot.status`) e issues bloqueadas (`hub.autopilot.digest`).
// Relida nos pushes de cada um; hub sem o método ou fora do ar = `undefined`.
import { useCallback, useEffect, useState } from "react";
import type { AutopilotStatus, ControlRoomProjects, MorningDigest } from "../protocol/hub-gen";
import { useTower } from "../state/tower";
import type { HubRoom } from "./attention";

export function useHubRoom(): HubRoom | undefined {
  const { api, hub } = useTower();
  const [projects, setProjects] = useState<ControlRoomProjects["projects"] | null>(null);
  const [pilot, setPilot] = useState<AutopilotStatus["sessions"]>([]);
  const [digest, setDigest] = useState<MorningDigest | null>(null);

  const loadProjects = useCallback(async () => {
    try {
      setProjects((await api.hubCall<ControlRoomProjects>("hub.control_room.projects")).projects);
    } catch {
      setProjects(null);
    }
  }, [api]);
  const loadPilot = useCallback(async () => {
    try {
      setPilot((await api.hubCall<AutopilotStatus>("hub.autopilot.status")).sessions);
    } catch {
      setPilot([]);
    }
    try {
      setDigest((await api.hubCall<MorningDigest[]>("hub.autopilot.digest", { limit: 1 }))[0] ?? null);
    } catch {
      setDigest(null);
    }
  }, [api]);

  useEffect(() => {
    if (!hub.connected) {
      setProjects(null);
      setPilot([]);
      setDigest(null);
      return;
    }
    void loadProjects();
    void loadPilot();
    return api.onHubPush((e) => {
      // `env.online`: o canal de um ambiente abriu ou fechou, e `env_online`
      // e a parada `ambiente_fora` dependem disso.
      if (e.event === "hub.control_room.changed" || e.event === "env.online") void loadProjects();
      if (e.event === "hub.autopilot.changed") void loadPilot();
    });
  }, [api, hub.connected, loadProjects, loadPilot]);

  if (!projects) return undefined;
  return {
    projects,
    pilotSessions: pilot.map((s) => ({ owner: s.owner, repo: s.repo, gh_number: s.gh_number })),
    blocked: digest?.projects ?? [],
    blockedAt: digest?.at ?? 0,
  };
}
