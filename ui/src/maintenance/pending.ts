// Selo da Manutenção na barra de atividades (#339): atualizações disponíveis
// (ambientes e este app) mais ambientes com limpeza pendente, a mesma conta
// da Visão geral (overviewRows.ts), do estado que o `useTower` já mantém.
import { useMaintenanceOverview } from "./overviewRows";

/** Pendências de manutenção (o selo some com 0). */
export function useMaintenancePending(): number {
  return useMaintenanceOverview().pending;
}
