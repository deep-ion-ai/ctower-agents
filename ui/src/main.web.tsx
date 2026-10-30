// Entrada do modo web (`vite build --mode web`, épico #205): a PWA servida
// pelo cthubd. O vite.config troca a entrada do index.html por esta, e o
// bundle não pode importar nada de `@tauri-apps/*`. A escolha da
// implementação da DaemonApi é feita aqui, no build (spec §10.1): o app
// desktop entra por main.tsx, com a `TauriDaemonApi`.
import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-sans/700.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./theme.css";
import { App } from "./App";
import { WebDaemonApi } from "./api/web";
import { WebGate } from "./web/WebGate";
import { registerServiceWorker } from "./web/pwa";

const api = new WebDaemonApi();
// PWA instalável (#203): o service worker guarda só o shell e mostra a Web Push.
registerServiceWorker();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <WebGate auth={api}>
      <App api={api} />
    </WebGate>
  </React.StrictMode>,
);
