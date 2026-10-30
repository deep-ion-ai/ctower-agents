import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { RecoveryStatus, SecretsList, Warning } from "../protocol/hub-gen";
import { dashboard, hubEnv, hubInfo, hubProject, hubProjectView, hubStatus, HUB_DASHBOARD } from "../test/fixtures";

const NOW = Date.now();
const DAY = 24 * 60 * 60 * 1000;
const CHAVE = "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ";
const PUB = "age1pendentepublicaxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

/** `hub.recovery.*` como o servidor: a chave privada só sai no `generate`; o `verify` confere a colada. */
function hubRecovery(initial: Partial<RecoveryStatus> = {}) {
  let st: RecoveryStatus = {
    configured: false,
    public_key: null,
    pending: false,
    pending_public_key: null,
    last_verified_at: null,
    reverify_days: 90,
    next_verify_at: null,
    reverify_due: false,
    ...initial,
  };
  const warnings = (): Warning[] =>
    st.configured
      ? st.reverify_due
        ? [{ kind: "recovery_reverify_due", message: "a chave de recuperação não é verificada há mais de 90 dias" }]
        : []
      : [{ kind: "recovery_not_configured", message: "chave de recuperação não configurada" }];
  return {
    status: () => structuredClone(st),
    warnings,
    generate: () => {
      st = { ...st, pending: true, pending_public_key: PUB };
      return { private_key: CHAVE, public_key: PUB, status: structuredClone(st) };
    },
    verify: (p: { key: string }) => {
      if (p.key !== CHAVE) throw new Error("a chave não confere com a chave de recuperação gerada");
      st = {
        ...st,
        configured: true,
        public_key: PUB,
        pending: false,
        pending_public_key: null,
        last_verified_at: NOW,
        next_verify_at: NOW + 90 * DAY,
        reverify_due: false,
      };
      return structuredClone(st);
    },
  };
}

const LIST: SecretsList = {
  status: { entries: 0, store_rev: 0, recovery_configured: false },
  items: [],
  delivery: [],
  rotate_pending: null,
};

function setup(rec: ReturnType<typeof hubRecovery>) {
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", () => hubStatus({ warnings: rec.warnings() }))
    .onHub("hub.projects.list", [hubProjectView(hubProject())])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null })
    .onHub("hub.secrets.list", LIST)
    .onHub("hub.recovery.status", () => rec.status())
    .onHub("hub.recovery.generate", () => rec.generate())
    .onHub("hub.recovery.verify", (p: any) => rec.verify(p));
  render(<App api={api} initial={{ name: "hub", section: "recovery" }} />);
  return { api, user: userEvent.setup() };
}

test("não configurada: mostra o estado e o aviso", async () => {
  const { user } = setup(hubRecovery());
  const sec = await screen.findByRole("region", { name: "Chave de recuperação" });
  expect(await within(sec).findByText("não configurada")).toBeInTheDocument();
  expect(within(sec).getByText(/se a máquina do hub se perder, os secrets não voltam/)).toBeInTheDocument();
  expect(within(sec).getByRole("button", { name: "Gerar chave de recuperação" })).toBeInTheDocument();
  // A seção pede atenção no painel lateral; o aviso da Visão geral leva de volta a ela.
  const row = () => screen.getByRole("button", { name: "Chave de recuperação" });
  await waitFor(() => expect(row()).toHaveAccessibleDescription("pede atenção"));
  await user.click(screen.getByRole("button", { name: "Visão geral" }));
  const avisos = await screen.findByRole("region", { name: /Avisos/ });
  expect(await within(avisos).findByText("chave de recuperação não configurada")).toBeInTheDocument();
  await user.click(within(avisos).getByRole("button", { name: "Verificar chave" }));
  expect(await screen.findByRole("region", { name: "Chave de recuperação" })).toBeInTheDocument();
  expect(row()).toHaveAttribute("aria-current", "true");
});

test("gerar → copiar → colar errada → colar certa: a chave some da tela", async () => {
  const { api, user } = setup(hubRecovery());
  const sec = await screen.findByRole("region", { name: "Chave de recuperação" });
  await user.click(await within(sec).findByRole("button", { name: "Gerar chave de recuperação" }));

  // A chave aparece uma vez, com a instrução e o aviso da dependência circular.
  const box = await within(sec).findByRole("group", { name: "Chave de recuperação gerada" });
  expect(within(box).getByText(CHAVE)).toBeInTheDocument();
  expect(within(box).getByText("Guarde no seu gerenciador de senhas.")).toBeInTheDocument();
  expect(within(box).getByText(/senha mestra do gerenciador de senhas e o recovery dele/)).toBeInTheDocument();
  expect(within(box).getByText(/pelo celular/)).toBeInTheDocument();
  expect(within(sec).getByText("gerada · falta verificar")).toBeInTheDocument();
  await user.click(within(box).getByRole("button", { name: "Copiar" }));
  expect(api.clipboard).toBe(CHAVE);
  expect(within(box).getByRole("button", { name: "Copiada" })).toBeInTheDocument();

  // Ao seguir para a verificação, a chave sai da tela.
  await user.click(within(box).getByRole("button", { name: "Já guardei: verificar" }));
  expect(document.body.textContent).not.toContain(CHAVE);
  const form = within(sec).getByRole("form", { name: "Verificar a chave de recuperação" });
  const campo = within(form).getByLabelText("Cole a chave de recuperação") as HTMLInputElement;
  expect(campo.type).toBe("password");
  expect(within(form).getByRole("button", { name: "Verificar" })).toBeDisabled();

  // Chave errada: erro, campo limpo, nada configurado.
  await user.type(campo, "AGE-SECRET-KEY-1ERRADA");
  await user.click(within(form).getByRole("button", { name: "Verificar" }));
  expect(await within(form).findByRole("alert")).toHaveTextContent("a chave não confere");
  expect(campo.value).toBe("");
  expect(within(sec).getByText("gerada · falta verificar")).toBeInTheDocument();

  // Chave certa (colada com espaços).
  await user.click(campo);
  await user.paste(`  ${CHAVE}\n`);
  await user.click(within(form).getByRole("button", { name: "Verificar" }));
  await waitFor(() => expect(api.hubCallsTo("hub.recovery.verify").slice(-1)[0]?.params).toEqual({ key: CHAVE }));
  expect(await within(sec).findByText(/A recuperação está configurada/)).toBeInTheDocument();
  expect(within(sec).getByText("configurada")).toBeInTheDocument();
  expect(within(sec).getByText(/próxima verificação em 90 dia\(s\)/)).toBeInTheDocument();
  expect(within(sec).queryByRole("form")).not.toBeInTheDocument();
  expect(document.body.textContent).not.toContain(CHAVE);
  expect(within(sec).getByRole("button", { name: "Verificar de novo" })).toBeInTheDocument();
  // O aviso some e, com ele, o "!" da seção.
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Chave de recuperação" })).toHaveAccessibleDescription(""),
  );
});

test("verificação vencida: mostra o estado e o lembrete", async () => {
  setup(
    hubRecovery({
      configured: true,
      public_key: PUB,
      last_verified_at: NOW - 100 * DAY,
      next_verify_at: NOW - 10 * DAY,
      reverify_due: true,
    }),
  );
  const sec = await screen.findByRole("region", { name: "Chave de recuperação" });
  expect(await within(sec).findByText("configurada · verificação vencida")).toBeInTheDocument();
  expect(within(sec).getByText(/refaça a verificação/)).toBeInTheDocument();
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "Chave de recuperação" })).toHaveAccessibleDescription("pede atenção"),
  );
});
