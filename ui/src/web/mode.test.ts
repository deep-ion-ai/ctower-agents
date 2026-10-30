import { HUB_PLACES } from "../hub/sections";
import { MAINTENANCE_PLACES } from "../maintenance/sections";
import { SETTINGS_PLACES } from "../settings/sections";
import { PLACES } from "../shell/places";
import { placeOnWeb, webPlaceLabel } from "./mode";

test("lugares da web: sem Terminal, Manutenção, Segurança do Hub nem as Configurações do desktop", () => {
  const ids = PLACES.filter(placeOnWeb).map((p) => p.id);
  expect(ids).not.toContain("screen:terminal");
  expect(ids).not.toContain("screen:maintenance");
  for (const p of MAINTENANCE_PLACES) expect(ids).not.toContain(p.id);
  expect(HUB_PLACES.filter(placeOnWeb).map((p) => p.id)).toEqual([
    "hub:overview",
    "hub:proposals",
    "hub:health",
    "hub:quarantine",
    "hub:projects",
    "hub:envs",
    "hub:web-sessions",
    "hub:devices",
  ]);
  expect(SETTINGS_PLACES.filter(placeOnWeb).map((p) => webPlaceLabel(p.label))).toEqual([
    "Configurações › Aparência",
    "Configurações › Acesso web",
  ]);
});
