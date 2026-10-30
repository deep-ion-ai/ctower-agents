import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../App";
import { FakeDaemonApi } from "../api/fake";
import type { BackupConfigView } from "../protocol/hub-gen";
import { dashboard, HUB_BACKUP, HUB_DOCS_BACKUP, hubEnv, hubInfo, hubProject, hubProjectView, hubStatus, HUB_DASHBOARD, recoveryStatus } from "../test/fixtures";

const SEGREDO = "chave-secreta-do-bucket";

const view = (p: Partial<BackupConfigView["remote"]> = {}, credentials = false): BackupConfigView => ({
  remote: { kind: "none", encrypt: true, bucket: "", prefix: "", region: "us-east-1", endpoint: "", drive_folder_id: "", ...p },
  status: {
    ...HUB_BACKUP,
    remote: { ...HUB_BACKUP.remote, kind: p.kind ?? "none", target: p.bucket ? `s3://${p.bucket}` : "", credentials_configured: credentials },
  },
  credential_fields: [],
  docs: HUB_DOCS_BACKUP,
});

function setup(initial = view(), status = hubStatus()) {
  let current = initial;
  const api = new FakeDaemonApi()
    .on("dashboard.get", dashboard({ projects: [] }))
    .withHub("ws://mac-mini:7879/ws")
    .onHub("hub.info", hubInfo())
    .onHub("hub.status", status)
    .onHub("hub.projects.list", [hubProjectView(hubProject())])
    .onHub("hub.envs.list", [hubEnv()])
    .onHub("hub.dashboard.get", HUB_DASHBOARD)
    .onHub("github.status", { source: "token", login: "hub-bot", error: null })
    .onHub("hub.secrets.list", { status: { entries: 0, store_rev: 1, recovery_configured: false }, items: [], delivery: [], rotate_pending: null })
    .onHub("hub.recovery.status", recoveryStatus({ configured: false, public_key: null, last_verified_at: null, next_verify_at: null }))
    .onHub("hub.backup.get", () => current)
    .onHub("hub.backup.set", (p: any) => {
      if (p.docs_keep !== undefined) {
        current = { ...current, docs: { ...current.docs, interval_hours: p.docs_interval_hours, keep: p.docs_keep, max_mb: p.docs_max_mb } };
        return current;
      }
      // O hub nunca devolve credenciais: só "configurado".
      current = view(p.remote, true);
      return current;
    })
    .onHub("hub.backup.test", (p: any) => (p.credentials?.secret_key === "errada" ? { ok: false, error: "HTTP 403" } : { ok: true }))
    .onHub("hub.backup.run", () => current);
  render(<App api={api} initial={{ name: "maintenance", section: "backup" }} />);
  return { api, user: userEvent.setup() };
}

test("escolhe S3, testa a conexão e salva; a credencial vai uma vez e não volta", async () => {
  const { api, user } = setup();
  await user.selectOptions(await screen.findByLabelText("Destino"), "s3");
  await user.type(screen.getByLabelText("Bucket"), "meu-bucket");
  await user.type(screen.getByLabelText("Access key (key id)"), "AKIA123");
  await user.type(screen.getByLabelText("Secret key (application key)"), SEGREDO);

  await user.click(screen.getByRole("button", { name: "Testar conexão" }));
  expect(await screen.findByText(/Conexão ok/)).toBeInTheDocument();
  expect(api.hubCallsTo("hub.backup.test")[0].params).toMatchObject({
    remote: { kind: "s3", bucket: "meu-bucket" },
    credentials: { access_key: "AKIA123", secret_key: SEGREDO },
  });

  await user.click(screen.getByRole("button", { name: "Salvar backup" }));
  expect(await screen.findByText("Configuração do backup salva.")).toBeInTheDocument();
  expect(api.hubCallsTo("hub.backup.set")[0].params).toMatchObject({ interval_hours: 6, keep: 28, remote: { kind: "s3", encrypt: true } });
  // Depois de salvo, o campo volta vazio e só diz "configurado".
  const secret = screen.getByLabelText("Secret key (application key)") as HTMLInputElement;
  expect(secret.value).toBe("");
  expect(secret.placeholder).toMatch(/configurado/);
  expect(document.body.textContent).not.toContain(SEGREDO);
});

test("teste de conexão que falha mostra o erro do provedor", async () => {
  const { user } = setup();
  await user.selectOptions(await screen.findByLabelText("Destino"), "s3");
  await user.type(screen.getByLabelText("Bucket"), "b");
  await user.type(screen.getByLabelText("Secret key (application key)"), "errada");
  await user.click(screen.getByRole("button", { name: "Testar conexão" }));
  expect(await screen.findByText(/Teste falhou: HTTP 403/)).toBeInTheDocument();
});

test("sem cifra avisa; Fazer backup agora chama o hub", async () => {
  const { api, user } = setup(view({ kind: "s3", bucket: "b" }, true));
  await user.click(await screen.findByLabelText(/Cifrar com age/));
  expect(screen.getByText(/sobe em texto claro/)).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Fazer backup agora" }));
  await waitFor(() => expect(api.hubCallsTo("hub.backup.run")).toHaveLength(1));
  expect(await screen.findByText("Backup feito.")).toBeInTheDocument();
});

const DOCS_OK = {
  ...HUB_DOCS_BACKUP,
  docs_dir: "/srv/ct-docs",
  newest_at: Date.now() - 3_600_000,
  newest_name: "docs-20261029-0300.tar.zst",
  last_ok_at: Date.now() - 3_600_000,
  last_size: 5 * 1024 * 1024,
  remote: { last_ok_at: Date.now() - 3_000_000, last_name: "docs-20261029-0300.tar.zst.age", last_size: 5 * 1024 * 1024, last_error: null, failures: 0 },
};

test("Docs dos projetos: último pacote local e remoto, agenda e retenção; backup agora só dos docs (#444)", async () => {
  const { api, user } = setup({ ...view(), docs: DOCS_OK });
  const bloco = await screen.findByRole("region", { name: "Docs dos projetos" });
  const facts = within(bloco).getByRole("group", { name: "Estado do backup dos docs" });
  expect(facts).toHaveTextContent("/srv/ct-docs");
  expect(facts).toHaveTextContent("docs-20261029-0300.tar.zst · 5,0 MB");
  expect(facts).toHaveTextContent("docs-20261029-0300.tar.zst.age");
  expect(facts).toHaveTextContent("a cada 24 horas");
  expect(facts).toHaveTextContent("7 pacotes mais novos");

  await user.click(within(bloco).getByRole("button", { name: "Fazer backup dos docs agora" }));
  await waitFor(() => expect(api.hubCallsTo("hub.backup.run")).toHaveLength(1));
  expect(api.hubCallsTo("hub.backup.run")[0].params).toEqual({ what: "docs" });
  expect(await within(bloco).findByText("Backup dos docs feito.")).toBeInTheDocument();

  // O do hub.db roda só a parte dele.
  await user.click(screen.getByRole("button", { name: "Fazer backup agora" }));
  await waitFor(() => expect(api.hubCallsTo("hub.backup.run")).toHaveLength(2));
  expect(api.hubCallsTo("hub.backup.run")[1].params).toEqual({ what: "hub" });

  const keep = within(bloco).getByLabelText("Pacotes dos docs mantidos");
  await user.clear(keep);
  await user.type(keep, "10");
  await user.click(within(bloco).getByRole("button", { name: "Salvar backup dos docs" }));
  await waitFor(() => expect(api.hubCallsTo("hub.backup.set")).toHaveLength(1));
  expect(api.hubCallsTo("hub.backup.set")[0].params).toEqual({ docs_interval_hours: 24, docs_keep: 10, docs_max_mb: 1024 });
  expect(await within(bloco).findByText("Configuração do backup dos docs salva.")).toBeInTheDocument();
  expect(within(bloco).getByRole("group", { name: "Estado do backup dos docs" })).toHaveTextContent("10 pacotes mais novos");
});

test("Docs dos projetos: avisos docs_backup_* no bloco; sem docs_dir, sem formulário nem botão ativo", async () => {
  setup(
    { ...view(), docs: { ...DOCS_OK, last_error: "disco cheio" } },
    hubStatus({ warnings: [{ kind: "docs_backup_failed", message: "backup dos docs falhou: disco cheio" }] }),
  );
  const bloco = await screen.findByRole("region", { name: "Docs dos projetos" });
  expect(await within(bloco).findByRole("list", { name: "Avisos do backup dos docs" })).toHaveTextContent("disco cheio");
  expect(within(bloco).getByRole("group", { name: "Estado do backup dos docs" })).toHaveTextContent("falhou: disco cheio");
  cleanup();

  setup();
  const vazio = await screen.findByRole("region", { name: "Docs dos projetos" });
  expect(vazio).toHaveTextContent("docs_dir não configurado no hub");
  expect(within(vazio).getByRole("button", { name: "Fazer backup dos docs agora" })).toBeDisabled();
  expect(within(vazio).queryByRole("form")).not.toBeInTheDocument();
});

test("hub anterior à #442 (sem docs): sem o bloco, e o backup agora vai sem what", async () => {
  const { docs: _, ...old } = view();
  const { api, user } = setup(old as BackupConfigView);
  await user.click(await screen.findByRole("button", { name: "Fazer backup agora" }));
  await waitFor(() => expect(api.hubCallsTo("hub.backup.run")).toHaveLength(1));
  expect(api.hubCallsTo("hub.backup.run")[0].params).toEqual({});
  expect(screen.queryByRole("region", { name: "Docs dos projetos" })).not.toBeInTheDocument();
});
