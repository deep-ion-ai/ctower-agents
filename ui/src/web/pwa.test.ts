// PWA (#203): deep link das notificações, o que o aparelho suporta e a
// chave VAPID no formato do applicationServerKey.
import { deepLinkTab, isIos, pushSupport, sameKey, vapidKeyBytes, type PushEnv } from "./pwa";

const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 Version/17.4 Mobile/15E148 Safari/604.1";
const ANDROID = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Chrome/126.0 Mobile Safari/537.36";
const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/17.4 Safari/605.1.15";

const env = (e: Partial<PushEnv> = {}): PushEnv => ({
  ua: ANDROID,
  touchPoints: 5,
  standalone: false,
  serviceWorker: true,
  pushManager: true,
  notification: true,
  permission: "default",
  ...e,
});

test("deep link: item, propostas, ambientes, dispositivos e desconhecido", () => {
  expect(deepLinkTab("?go=item&project=deep-ion-ai/demo&n=5")).toEqual({
    id: "project:deep-ion-ai/demo#5",
    title: "#5",
    screen: { name: "project", owner: "deep-ion-ai", repo: "demo", item: { ghNumber: 5 } },
  });
  expect(deepLinkTab("?go=hub:proposals")?.screen).toEqual({ name: "hub", section: "proposals" });
  expect(deepLinkTab("?go=hub:envs")?.screen).toEqual({ name: "hub", section: "envs" });
  expect(deepLinkTab("?go=hub:devices")?.screen).toEqual({ name: "hub", section: "devices" });
  expect(deepLinkTab("?go=control")?.screen).toEqual({ name: "control" });
  // Detalhe do ambiente (#23); seção desconhecida = Conexão.
  expect(deepLinkTab("?go=env&env=daemon-wsl&section=hub")?.screen).toEqual({ name: "env", env: "daemon-wsl", section: "hub" });
  expect(deepLinkTab("?go=env&env=daemon-wsl&section=nova")?.screen).toEqual({ name: "env", env: "daemon-wsl" });
  expect(deepLinkTab("?go=env")).toBeNull();
  expect(deepLinkTab("")).toBeNull();
  expect(deepLinkTab("?go=item&project=sem-barra&n=5")).toBeNull();
  expect(deepLinkTab("?go=item&project=o/r&n=abc")).toBeNull();
  expect(deepLinkTab("?go=terminal")).toBeNull();
});

test("deep link do projeto numa aba e numa seção (#17)", () => {
  const base = { name: "project", owner: "deep-ion-ai", repo: "demo" };
  // Sem `view` (ou desconhecida): Itens, o endereço de sempre.
  expect(deepLinkTab("?go=project&project=deep-ion-ai/demo")).toEqual({ screen: base });
  expect(deepLinkTab("?go=project&project=deep-ion-ai/demo&view=nova")).toEqual({ screen: base });
  expect(deepLinkTab("?go=project&project=deep-ion-ai/demo&view=sessions")?.screen).toEqual({ ...base, view: "sessions" });
  expect(deepLinkTab("?go=project&project=deep-ion-ai/demo&view=settings&section=rules")?.screen).toEqual({
    ...base,
    view: "settings",
    section: "rules",
  });
  // Seção desconhecida ou fora das Configurações: sai do endereço.
  expect(deepLinkTab("?go=project&project=deep-ion-ai/demo&view=settings&section=x")?.screen).toEqual({
    ...base,
    view: "settings",
  });
  expect(deepLinkTab("?go=project&project=deep-ion-ai/demo&view=overview&section=rules")?.screen).toEqual({
    ...base,
    view: "overview",
  });
  // GitLab: o `owner` com barras (divide na última).
  expect(deepLinkTab("?go=project&project=grupo/sub/app&view=items")?.screen).toEqual({
    name: "project",
    owner: "grupo/sub",
    repo: "app",
  });
  expect(deepLinkTab("?go=project&project=sem-barra")).toBeNull();
  expect(deepLinkTab("?go=project")).toBeNull();
});

test("iOS fora da tela de início pede para instalar; Android pode ativar", () => {
  expect(isIos(IPHONE, 5)).toBe(true);
  // iPad no modo desktop: "Macintosh" com toque.
  expect(isIos(MAC, 5)).toBe(true);
  expect(isIos(MAC, 0)).toBe(false);
  expect(pushSupport(env({ ua: IPHONE }))).toBe("ios-home-screen");
  // Na tela de início (standalone), o iOS 16.4+ tem Web Push.
  expect(pushSupport(env({ ua: IPHONE, standalone: true }))).toBe("ok");
  // iOS antigo, mesmo instalado: sem PushManager.
  expect(pushSupport(env({ ua: IPHONE, standalone: true, pushManager: false }))).toBe("unsupported");
  expect(pushSupport(env())).toBe("ok");
  expect(pushSupport(env({ permission: "denied" }))).toBe("denied");
  expect(pushSupport(env({ serviceWorker: false }))).toBe("unsupported");
});

test("chave VAPID base64url vira os 65 bytes do ponto P-256", () => {
  const raw = new Uint8Array(65).map((_, i) => (i === 0 ? 4 : (i * 37) % 256));
  const b64url = btoa(String.fromCharCode(...raw))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
  expect(Array.from(vapidKeyBytes(b64url))).toEqual(Array.from(raw));
});

test("inscrição com outra chave VAPID é refeita", () => {
  const key = new Uint8Array([4, 1, 2]);
  expect(sameKey(null, key)).toBe(true);
  expect(sameKey(new Uint8Array([4, 1, 2]).buffer, key)).toBe(true);
  expect(sameKey(new Uint8Array([4, 1, 3]).buffer, key)).toBe(false);
  expect(sameKey(new Uint8Array([4, 1]).buffer, key)).toBe(false);
});

test("deep link do antigo Hub › Projetos (#18): as Configurações do projeto ou o índice de Projetos", () => {
  expect(deepLinkTab("?go=hub:projects")).toEqual({ screen: { name: "project" } });
  expect(deepLinkTab("?go=hub:projects&project=deep-ion-ai/demo")).toEqual({
    screen: { name: "project", owner: "deep-ion-ai", repo: "demo", view: "settings" },
  });
  // GitLab: o `owner` tem barras; corta na última.
  expect(deepLinkTab("?go=hub:projects&project=grupo/projeto/app")?.screen).toEqual({
    name: "project",
    owner: "grupo/projeto",
    repo: "app",
    view: "settings",
  });
  expect(deepLinkTab("?go=hub:projects&project=sem-barra")).toEqual({ screen: { name: "project" } });
});
