// A grade de sessões saiu da tela inicial da Sala de controle (#357): fica
// nas visões "Rodando" e "Precisa de você", abertas pelos chips do topo.
import { screen } from "@testing-library/react";
import userEvent, { type UserEvent } from "@testing-library/user-event";
import { HEALTH_LABEL } from "../control-room/Health";

/** Abre a visão de um chip do topo: "rodando" (padrão) ou "pedem você". */
export async function showSessions(user: Pick<UserEvent, "click">, chip: "rodando" | "pede" = "rodando") {
  const name = chip === "rodando" ? /^\d+ rodando$/ : /^\d+ pedem? você$/;
  await user.click(await screen.findByRole("button", { name, pressed: false }));
}

/** Abre (se fechado) o painel de saúde do topo e devolve a região com o hub e os ambientes. */
export async function openHealth(): Promise<HTMLElement> {
  const btn = await screen.findByRole("button", { description: HEALTH_LABEL });
  if (btn.getAttribute("aria-expanded") !== "true") await userEvent.setup().click(btn);
  return screen.getByRole("region", { name: HEALTH_LABEL });
}
