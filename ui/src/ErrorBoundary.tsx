import { Component, type ErrorInfo, type ReactNode } from "react";
import { rawConsole, uiLog } from "./logs/uiLog";

/**
 * Uma exceção ao renderizar uma tela não pode deixar a janela em branco: mostra o erro
 * (que ajuda a diagnosticar hub ou daemon desatualizado, por exemplo) e deixa tentar de novo.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // No log da UI (#65) com a pilha; o console recebe sem passar de novo pelo log.
    rawConsole.error("Erro ao renderizar a tela:", error, info.componentStack);
    uiLog.add("error", "ui.react", `Erro ao renderizar a tela: ${error.message}`, {
      ...(error.stack ? { stack: error.stack } : {}),
      ...(info.componentStack ? { component_stack: info.componentStack.trim() } : {}),
    });
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <main className="page">
        <div role="alert" className="banner">
          <p>
            <strong>Esta tela falhou ao abrir.</strong> Pode ser o hub ou o daemon do ambiente desatualizado em
            relação ao app: atualize o daemon do ambiente em Manutenção › Atualizações › Ambientes (a atualização
            reinicia o daemon) e, se a tela é do hub, o hub (cthubd) no servidor dele.
          </p>
          <p className="mono">{error.message}</p>
          <button type="button" className="btn" onClick={() => this.setState({ error: null })}>
            Tentar de novo
          </button>
        </div>
      </main>
    );
  }
}
