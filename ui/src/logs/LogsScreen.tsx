// Tela Logs (#65) no endereço da Manutenção (`logsTab`): a fonte vem do
// endereço e trocar a fonte troca o endereço da mesma aba.
import { logsSource, logsTab, type MaintenanceScreen } from "../maintenance/sections";
import { useNav } from "../state/nav";
import { LogsView, useDefaultSource } from "./LogsView";

export function LogsSection({ screen }: { screen: MaintenanceScreen }) {
  const nav = useNav();
  const fallback = useDefaultSource();
  const source = logsSource(screen) ?? fallback;
  return <LogsView source={source} onSource={(s) => nav.openTab({ ...logsTab(s), id: nav.current })} />;
}

/**
 * Na web a Manutenção não existe (#340), mas os Logs sim: a mesma tela,
 * sem o painel lateral das seções do desktop.
 */
export function WebLogsScreen({ screen }: { screen: MaintenanceScreen }) {
  return (
    <main className="page logs-page" aria-labelledby="logs-h1">
      <h1 id="logs-h1">Logs</h1>
      <LogsSection screen={screen} />
    </main>
  );
}
