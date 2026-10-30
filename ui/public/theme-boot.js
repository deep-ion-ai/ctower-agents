// Tema antes do primeiro quadro (#290), para não piscar o tema errado: o
// ui.json só chega à UI depois, pelo invoke. Vale a última preferência
// aplicada (state/theme.ts) ou a que o app nativo leu do ui.json.
(function () {
  var pref = null;
  try {
    pref = localStorage.getItem("ct.theme");
  } catch (e) {}
  pref = pref || window.__CT_THEME__ || "dark";
  var light =
    pref === "light" || (pref === "system" && !window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = light ? "light" : "dark";
})();
