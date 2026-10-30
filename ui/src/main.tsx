import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-sans/700.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "@xterm/xterm/css/xterm.css";
import "./theme.css";
import { App } from "./App";
import { TauriDaemonApi } from "./api/daemon";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App api={new TauriDaemonApi()} />
  </React.StrictMode>,
);
