import type { Tower } from "../state/tower";

/**
 * Hub e ambientes em modo de teste (#176): `test_mode` no `hub.info` e no
 * `env.info`. Ausente = falso (daemon anterior à #176).
 */
export interface TestTargets {
  hub: string | null;
  envs: string[];
}

export function testTargets(t: Pick<Tower, "envs" | "infos" | "hub">): TestTargets | null {
  const hub = t.hub.info?.test_mode ? t.hub.info.name : null;
  const envs = t.envs.filter((e) => t.infos[e.id]?.test_mode).map((e) => e.name);
  return hub || envs.length > 0 ? { hub, envs } : null;
}

/** Texto do selo: diz quem está em modo de teste. */
export function testDescription(t: TestTargets): string {
  const parts: string[] = [];
  if (t.hub) parts.push(`hub ${t.hub}`);
  if (t.envs.length === 1) parts.push(`ambiente ${t.envs[0]}`);
  else if (t.envs.length > 1) parts.push(`ambientes ${t.envs.join(", ")}`);
  return `Modo de teste: ${parts.join(" e ")}.`;
}

export const TEST_TITLE_PREFIX = "[TESTE] ";
