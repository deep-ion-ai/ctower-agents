# Acesso remoto ao hub: Tailscale e Cloudflare Tunnel com Access

Como abrir a UI web do hub (a PWA, épico #205) no celular, fora da máquina do hub, por dois caminhos: o **`tailscale serve`** (o celular entra na tailnet) e o **Cloudflare Tunnel com Access** (o celular só precisa de um navegador). Este documento cobre a configuração dos dois proxies, o endurecimento, a privacidade, o iOS e o roteiro de verificação de cada caminho (#204).

O comportamento do hub descrito aqui (cookie, bloqueio, conferência do JWT, recusas) está em [`docs/hub.md`](hub.md), seção "UI web (`[web]`)". O desenho está na spec `docs/superpowers/specs/2026-10-28-pwa-celular.md` (§3, §7.4 e §11).

## 1. Visão geral

```
 celular (PWA)                                   máquina do hub
 ─────────────                                   ──────────────
  navegador ──HTTPS── tailscale serve ───────┐
            ──HTTPS── Cloudflare (Access) ── cloudflared ──┤
                                                            ▼  http://127.0.0.1:7879
                                                          cthubd
                                            /, /assets/*, /web/login|logout|me, /web/ws
```

- O `cthubd` não ganha porta nova nem TLS próprio: o proxy roda na máquina do hub e fala com `http://127.0.0.1:7879` (a porta padrão; `port` ou `CTHUB_PORT` mudam).
- A identidade do proxy é **uma porta a mais**, não substitui o login: o primeiro acesso de cada aparelho ainda pede o token de UI do hub (`~/.config/cthub/ui.token`), que vira o cookie `ct_web`.
- A PWA só acompanha e decide (status, prioridade, propostas do gestor). Conversar com a sessão do agente é papel do Remote Control, no app Claude ou em `claude.ai/code/<id>`.

| | Tailscale (`tailscale serve`) | Cloudflare Tunnel com Access |
|---|---|---|
| O celular precisa de | app Tailscale, logado na tailnet | só o navegador |
| Endereço | `https://<máquina>.<tailnet>.ts.net` | `https://hub.exemplo.com` (domínio seu na Cloudflare) |
| Identidade conferida pelo hub | `Tailscale-User-Login` (`[web.tailscale]`) | JWT `Cf-Access-Jwt-Assertion` (`[web.cloudflare]`) |
| TLS termina | no próprio hub (o tráfego vai cifrado ponta a ponta pelo WireGuard) | na Cloudflare, que vê o tráfego decifrado |
| Além da PWA | o mesmo caminho serve o app desktop e os ambientes (pelo IP Tailscale, veja §7) | só as rotas web: `/ws`, `/sync/*` e `/agent/v1/*` respondem `404` |
| Quando usar | o padrão: você já usa Tailscale entre as máquinas | aparelho sem Tailscale, ou rede que bloqueia o Tailscale |

Os dois podem conviver: configure as duas seções (`[web.tailscale]` e `[web.cloudflare]`) e ponha as duas origens em `[web] origins`. Com só uma das seções, o outro caminho recebe `403` em todas as rotas web.

## 2. Pré-requisitos no hub

1. Ligue a web no `~/.config/cthub/config.toml` (o hub recusa chaves desconhecidas, então copie os nomes como estão):

   ```toml
   [web]
   enabled = true                 # sem isso, /, /assets/* e /web/* respondem 404
   origins = []                   # preenchido nas seções 3 e 4
   session_days = 30              # validade absoluta do cookie ct_web
   idle_days = 7                  # dias sem uso até a sessão vencer
   ```

2. Deixe o `bind` no padrão (`127.0.0.1`, ou sem a chave). Veja §5 e §7 para o caso de o hub também precisar escutar na LAN ou no IP Tailscale.
3. Reinicie o hub para ler o config: `systemctl --user restart cthubd` (Linux) ou `launchctl kickstart -k gui/$(id -u)/ai.deep-ion.cthubd` (macOS). Um proxy configurado pela metade (sem `aud`, sem `emails`, sem `logins`) ou `session_days`/`idle_days` igual a `0` impedem o hub de subir: confira o log (`journalctl --user -u cthubd` ou `~/Library/Logs/cthubd.log`).
4. Tenha o token de UI à mão (`cat ~/.config/cthub/ui.token`), para digitar uma vez em cada aparelho. Ele não fica guardado na PWA.

## 3. Tailscale (`tailscale serve`)

### 3.1 Tailnet e HTTPS

1. Instale o Tailscale na máquina do hub e no celular e entre com a mesma conta (ou compartilhe a máquina com a conta do celular).
2. No painel do Tailscale (**DNS**), ligue o **MagicDNS** e os **HTTPS Certificates**. Sem os certificados, o `tailscale serve` pede para ligá-los.
3. Anote o nome da máquina: `tailscale status` (ou o painel) mostra `<máquina>.<tailnet>.ts.net`.
4. Descubra o login que o Tailscale vai mandar ao hub: `tailscale status --json | grep LoginName` (ou `tailscale whois <ip-tailscale-do-celular>`). Contas do GitHub aparecem como `usuario@github`; contas Google e Microsoft, como o e-mail.

### 3.2 Publicar o hub na tailnet

Na máquina do hub:

```bash
tailscale serve --bg --https=443 http://127.0.0.1:7879
tailscale serve status        # confere: https://<máquina>.<tailnet>.ts.net -> http://127.0.0.1:7879
```

O atalho `tailscale serve --bg 7879` faz o mesmo (porta nua = `http://127.0.0.1:7879`). Use `127.0.0.1`, não `localhost`: o hub escuta só no IPv4 do loopback. O `--bg` deixa a configuração persistente, inclusive depois de reiniciar o Tailscale. Para desfazer: `tailscale serve --https=443 off` (ou `tailscale serve reset`, que apaga toda a configuração do serve).

- **Não use o Funnel** (`tailscale funnel`): ele publica na internet e não manda o `Tailscale-User-Login`, então toda rota web responde `403` com `[web.tailscale]` configurado.
- Aparelhos com **tag** (`tag:...`) também não recebem os headers de identidade: o celular precisa estar logado com um usuário.
- O WebSocket (`/web/ws`) passa pelo `serve` sem configuração extra.

### 3.3 Config do hub

```toml
[web]
enabled = true
origins = ["https://<máquina>.<tailnet>.ts.net"]   # sem barra no fim; com --https=8443, inclua ":8443"

[web.tailscale]
logins = ["usuario@github"]                        # o LoginName da §3.1; sem diferença de maiúsculas
```

Reinicie o hub. O `Tailscale-User-Login` só vale com a conexão vinda do loopback (o `tailscale serve` fala de `127.0.0.1`) e sem nenhum header `Cf-*`. Quem chega direto ao IP Tailscale ou da LAN com o header escrito à mão é ignorado (e recusado com `403`). O `serve` também apaga esses headers se o cliente os mandar.

## 4. Cloudflare Tunnel com Access

Requer um domínio na Cloudflare (o exemplo usa `hub.exemplo.com`) e o Zero Trust ligado na conta (o plano gratuito basta para uso pessoal).

### 4.1 Túnel

Na máquina do hub, com o `cloudflared` instalado (`brew install cloudflared` no macOS; pacote da Cloudflare no Linux):

```bash
cloudflared tunnel login                              # abre o navegador e grava ~/.cloudflared/cert.pem
cloudflared tunnel create cthub                       # grava ~/.cloudflared/<UUID>.json (credencial do túnel)
cloudflared tunnel route dns cthub hub.exemplo.com    # cria o CNAME hub.exemplo.com -> <UUID>.cfargotunnel.com
```

`~/.cloudflared/config.yml`:

```yaml
tunnel: <UUID>
credentials-file: /Users/<voce>/.cloudflared/<UUID>.json   # no Linux: /home/<voce>/.cloudflared/<UUID>.json

ingress:
  # O hub já responde 404 nessas rotas quando a requisição traz Cf-Connecting-IP;
  # o túnel corta antes, como segunda barreira.
  - hostname: hub.exemplo.com
    path: ^/(ws|sync|agent)(/|$)
    service: http_status:404
  - hostname: hub.exemplo.com
    service: http://127.0.0.1:7879
  - service: http_status:404                               # obrigatória: a última regra pega o resto
```

O WebSocket (`/web/ws`) passa pelo túnel sem configuração extra. Confira as regras antes de subir:

```bash
cloudflared tunnel ingress validate
cloudflared tunnel ingress rule https://hub.exemplo.com/web/ws    # deve cair em http://127.0.0.1:7879
cloudflared tunnel ingress rule https://hub.exemplo.com/sync/push # deve cair em http_status:404
cloudflared tunnel run cthub                                      # em primeiro plano, para testar
```

Como serviço:

- **macOS:** `cloudflared service install` (sem `sudo`) cria um LaunchAgent do usuário que lê `~/.cloudflared/config.yml` e, como o `cthubd`, só sobe depois do login. Com `sudo`, vira LaunchDaemon e lê `/etc/cloudflared/config.yml` (copie o config e a credencial para lá).
- **Linux:** `sudo cloudflared service install` cria a unidade systemd `cloudflared`, que lê `/etc/cloudflared/config.yml`.

### 4.2 Aplicação no Access

No painel Zero Trust, **Access → Applications → Add an application → Self-hosted**:

1. **Domínio:** `hub.exemplo.com`, caminho vazio (a aplicação cobre o domínio inteiro).
2. **Session Duration:** quanto tempo o login no Access vale (ex.: 24 h ou 1 semana). Vencido, o Access pede o login de novo antes de deixar a requisição chegar ao hub. É independente do cookie `ct_web` (`session_days`/`idle_days`).
3. **Política:** ação **Allow**, regra **Include → Emails** com os seus e-mails. Nada de `Everyone` nem domínio inteiro.
4. **Identity provider:** o "One-time PIN" (código por e-mail) basta; Google ou GitHub também servem.
5. Salve e copie o **Application Audience (AUD) Tag** da aplicação. O **team domain** fica em **Settings → Custom Pages** (ou **Team name and domain**): `<equipe>.cloudflareaccess.com`.

### 4.3 Config do hub

```toml
[web]
enabled = true
origins = ["https://hub.exemplo.com"]

[web.cloudflare]
team_domain = "<equipe>.cloudflareaccess.com"   # com ou sem https://
aud = "<Application Audience (AUD) Tag>"
emails = ["eu@exemplo.com"]                     # os mesmos da política; sem diferença de maiúsculas
```

Reinicie o hub. Em cada rota web, ele confere o `Cf-Access-Jwt-Assertion`: RS256, assinado por uma das chaves de `https://<team_domain>/cdn-cgi/access/certs` (cache de 1 h), `iss = https://<team_domain>`, `aud` com o AUD configurado, `exp`/`nbf` válidos e `email` na lista. Os e-mails ficam nos dois lugares de propósito: um erro na política do Access não abre o hub. O hub precisa alcançar `https://<team_domain>` (HTTPS de saída) para buscar as chaves.

Com `[web.cloudflare]` e sem `[web.push] subject`, o `sub` do JWT VAPID da Web Push é o primeiro e-mail de `emails`.

## 5. Endurecimento

- **Só o loopback atrás de proxy.** Sem `bind` (ou com `bind = "127.0.0.1"`), o `cthubd` escuta só em `127.0.0.1:<port>`; o loopback fica sempre no ar e `bind` acrescenta um endereço (`ct_http::serve`). Com o proxy na mesma máquina, nada mais precisa alcançar a porta. Um `bind` na LAN ou no IP Tailscale só é necessário para os ambientes e o app desktop em outras máquinas (§7). Com `bind` explícito, as rotas web também respondem nesse endereço, em HTTP; com `[web.tailscale]` ou `[web.cloudflare]` configurado, elas recusam com `403` (o `Tailscale-User-Login` de fora do loopback é ignorado e o JWT não se forja), então configure o proxy antes de abrir o `bind`.
- **Token de UI como segunda camada.** Mesmo com a identidade do proxy válida, o cookie `ct_web` só nasce com o token de UI. Sem cookie, o `/web/ws` responde `401`.
- **O que o hub recusa vindo pela Cloudflare:** com `Cf-Connecting-IP` na requisição, `/ws`, `/sync/*` e `/agent/v1/*` respondem `404`, com ou sem `[web.cloudflare]`. O token de UI não abre o `/ws` pelo túnel, e os tokens de sync e de agente não servem por ali. A regra `http_status:404` do `config.yml` (§4.1) repete isso no túnel.
- **Origin.** O upgrade do `/web/ws` e todo `POST /web/*` exigem `Origin` igual a um item de `origins` (`403` se ausente ou outro). Liste só as origens que você usa.
- **Bloqueio do login.** Da 5ª falha seguida em diante, o `/web/login` fica bloqueado por 1 min, dobrando até 1 h (`429` com `Retry-After`). O contador é global: atrás de proxy, todos chegam de `127.0.0.1`. O bloqueio não afeta o `/ws`, o sync nem as sessões web abertas.
- **Revogar sessões web.** Pela UI: **Hub › Sessões web** (desktop ou PWA) revoga uma ou todas. Pela linha de comando, na máquina do hub, com o hub no ar ou parado:

  ```bash
  cthubd web revoke-all     # imprime quantas sessões ativas foram revogadas
  ```

  Revogar uma sessão apaga também as inscrições de Web Push dela. **Trocar o `ui.token` não derruba as sessões web:** depois de trocar, revogue todas. Aparelho perdido: revogue as sessões, troque o `ui.token` e, na Cloudflare, revogue as sessões do Access (**Access → Users** › Revoke) ou tire o e-mail da política; no Tailscale, remova o aparelho da tailnet.

## 6. Privacidade

- **Cloudflare:** o TLS termina na Cloudflare, que vê o tráfego decifrado entre o celular e o hub: o dashboard, os títulos dos itens, as decisões e, na fase 2, o fim do transcript (`transcript.tail`). O token de UI também passa por ela no login. Se isso não serve, use o Tailscale.
- **Tailscale:** o tráfego vai cifrado pelo WireGuard até a máquina do hub, e o certificado HTTPS é da própria máquina; a coordenação do Tailscale vê só quais aparelhos falam com quais.
- **Remote Control:** com o Remote Control ligado numa sessão, o transcript fica nos servidores da Anthropic enquanto a sessão está conectada, qualquer que seja o caminho da PWA. A PWA só mostra o link `claude.ai/code/<id>`.
- **Web Push:** o conteúdo vai cifrado pelo serviço de push do navegador (Apple, Google, Mozilla), que vê só o endpoint e o tamanho. O corpo leva o projeto, o número, o título do item e o tipo do evento.

## 7. Relação com o hub na LAN (#252)

A issue #252 tratou dos ambientes (`ctowerd`) em outras máquinas que enviam ao hub pela LAN (ex.: o Mac mini enviando ao hub no WSL). Os dois casos usam a mesma porta do `cthubd` (`7879`), mas por caminhos e com credenciais diferentes:

| Quem | Caminho | Endereço | Credencial |
|---|---|---|---|
| Ambiente (`ctowerd`) em outra máquina: sync e pull da config | direto na porta do hub, sem proxy | `hub_url = "http://<ip-da-lan-ou-tailscale>:7879"` (exige `bind` nesse IP ou `0.0.0.0`) | token de sync (`cthubd add-env`, `~/.config/ctower/hub.token`) e, para o gestor, o token de agente (`/agent/v1/*`) |
| Ambiente na mesma máquina do hub | loopback | `hub_url = "http://127.0.0.1:7879"` | token de sync |
| App desktop | direto na porta do hub | `http://<máquina>:7879` (`/ws`) | token de UI no `Authorization` |
| PWA no celular | `tailscale serve` ou `cloudflared` → `127.0.0.1:7879` | `https://<máquina>.<tailnet>.ts.net` ou `https://hub.exemplo.com` (porta 443 do proxy) | identidade do proxy + cookie `ct_web` (criado com o token de UI no `/web/login`) |

- O `ctowerd` não fala HTTPS com o hub (o cliente dele é só HTTP), então o `hub_url` é sempre `http://` direto na porta, nunca a URL do `tailscale serve` nem a da Cloudflare (que, além disso, responde `404` no `/sync/*`).
- Na LAN, o sync e o `/ws` do desktop vão em HTTP sem TLS: os tokens passam em claro, o que só é aceitável numa rede doméstica confiável. Fora de casa, use o IP Tailscale no `hub_url` e no app desktop (`bind = "100.x.y.z"`), que vai cifrado pelo WireGuard.
- O `bind` aberto para os ambientes não precisa abrir a web: com `[web.tailscale]` ou `[web.cloudflare]` configurado, as rotas web pela LAN ou pelo IP Tailscale direto respondem `403` (§5).

O `docs/hub.md` (seção 3, "Ligar um ambiente ao hub", e seção 7, "Segurança") remete a este documento.

## 8. iPhone e iPad

- A Web Push exige **iOS 16.4 ou mais novo** e a PWA **adicionada à tela de início**: no Safari, **Compartilhar › Adicionar à Tela de Início**, e depois abrir pelo ícone (fora do Safari).
- O pedido de permissão precisa vir de um toque: em **Hub › Dispositivos**, "Ativar notificações neste dispositivo". No Safari fora da tela de início, a tela mostra essa instrução no lugar do botão.
- O app da tela de início tem cookies próprios, separados do Safari: faça o login (token de UI) de novo dentro dele. Com a Cloudflare, o login do Access também é refeito ali.
- Remover o ícone da tela de início perde a inscrição; o hub a apaga quando o serviço de push responder `404`/`410`.

## 9. Roteiro de verificação

Execute cada caminho e registre o resultado na issue #204 (data, versão do hub em `/healthz`, aparelho e versão do sistema, o que passou e o que não passou). Os comandos `curl` rodam na máquina do hub, salvo indicação.

### 9.1 Base (os dois caminhos)

1. `curl -s http://127.0.0.1:7879/healthz` → `{"ok":true,"role":"hub",...}`. Anote `version` e `commit`.
2. `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7879/web/me` → com proxy configurado, `403` (sem identidade); sem proxy, `200`. Com `enabled = false`, `404`.
3. Sem `bind` explícito: `lsof -nP -iTCP:7879 -sTCP:LISTEN` (macOS) ou `ss -ltnp | grep 7879` (Linux) mostra só `127.0.0.1:7879`.

### 9.2 Tailscale

1. `tailscale serve status` mostra `https://<máquina>.<tailnet>.ts.net` → `http://127.0.0.1:7879`.
2. No celular, com o Tailscale ligado, abra `https://<máquina>.<tailnet>.ts.net`: a tela de login da PWA aparece com cadeado válido.
3. Com o Tailscale **desligado** no celular, o endereço não abre.
4. Digite um token errado: "token inválido". No log do hub, `/web/login: token errado (<login>)` com o seu login do Tailscale.
5. Digite o token de UI: o shell abre. Recarregue a página: continua logado (cookie gravado). **Hub › Sessões web** mostra a sessão com a identidade `<login>` e o agente do navegador.
6. Mude um status ou uma prioridade pela PWA e confira no app desktop.
7. Tire o login de `[web.tailscale] logins` (ponha outro), reinicie o hub e recarregue: `403`, e no log `Tailscale-User-Login <login> fora da lista`. Volte o login.
8. De outra máquina da tailnet (ou da LAN), com o hub com `bind` explícito, `curl -s -o /dev/null -w '%{http_code}\n' -H 'Tailscale-User-Login: <login>' http://<ip>:7879/web/me` → `403` (log: "conexão de fora do loopback"). Sem `bind`, a conexão é recusada.
9. iOS (§8): adicione à tela de início, abra pelo ícone, faça o login, ative as notificações em **Hub › Dispositivos** e use **Testar**: a notificação chega; o toque abre a PWA.
10. Revogue a sessão em **Hub › Sessões web** (pelo desktop): o `/web/ws` da PWA cai em até 30 s (código `4401`), a PWA recarregada mostra o login, e o aparelho some de **Hub › Dispositivos**.

### 9.3 Cloudflare Tunnel com Access

1. `cloudflared tunnel ingress validate` passa e `cloudflared tunnel ingress rule` mostra as rotas da §4.1.
2. Numa janela anônima, abra `https://hub.exemplo.com`: o Access pede o login antes de qualquer página do hub.
3. Entre com um e-mail **fora** da política: o Access recusa (o hub nem recebe a requisição).
4. Entre com o seu e-mail: a tela de login da PWA aparece. Faça o login com o token de UI; o shell abre e **Hub › Sessões web** mostra a identidade com o seu e-mail.
5. Recarregue e navegue: o `/web/ws` conecta (dados aparecem e atualizam).
6. As rotas fechadas, do computador logado no Access (cookie `CF_Authorization` do navegador) ou com `cloudflared access curl`: `https://hub.exemplo.com/ws`, `/sync/config` e `/agent/v1/overview` → `404`.
7. Direto no hub, simulando o túnel: `curl -s -o /dev/null -w '%{http_code}\n' -H 'Cf-Connecting-IP: 203.0.113.9' http://127.0.0.1:7879/sync/config` → `404` (log: "recusado: requisição vinda pela Cloudflare").
8. Direto no hub, com um JWT inválido: `curl -s -o /dev/null -w '%{http_code}\n' -H 'Cf-Access-Jwt-Assertion: x.y.z' http://127.0.0.1:7879/web/me` → `403` (log: `rota web recusada (Cloudflare Access): JWT ilegível...`).
9. Troque o `aud` do config por outro valor, reinicie e recarregue a PWA: `403` (log: `JWT recusado: InvalidAudience`). Volte o valor.
10. Instalação: adicione a PWA à tela de início (iOS, §8) e confira que o ícone e o nome "Control Tower" aparecem e que ela abre em tela cheia. Ative e teste as notificações como na §9.2.
11. Depois da **Session Duration** do Access, a PWA pede o login do Access de novo; o cookie `ct_web` continua valendo depois dele.
12. `cthubd web revoke-all` na máquina do hub: imprime `1 sessão(ões) web ativa(s) revogada(s)` (ou mais), e a PWA recarregada mostra o login.

## 10. Solução de problemas

| Sintoma | Causa provável | O que fazer |
|---|---|---|
| Login aceito, mas a página volta para o login (o `/web/me` diz `authenticated: false`) | o cookie `ct_web` é `Secure` e não grava em HTTP (ex.: `http://<ip-da-lan>:7879`); o Safari pode recusá-lo até em `http://127.0.0.1` | abra pelo endereço HTTPS do proxy. No próprio hub, sem proxy configurado, use Chrome ou Firefox em `http://127.0.0.1:7879` |
| `403` "origin não permitido" no login ou no `/web/ws`; log `/web/login recusado: Origin ... fora da lista` | a origem do navegador não está em `[web] origins` (barra no fim é ignorada; esquema e porta contam) | copie a origem do log para `origins` e reinicie o hub. Com `origins` preenchido, `http://127.0.0.1:<port>` deixa de valer, salvo se listado |
| `403` "acesso negado" em todas as rotas web | proxy configurado e nenhuma identidade válida: caminho do Tailscale com só `[web.cloudflare]` (ou o contrário), login fora de `logins`, aparelho com tag, Funnel, acesso direto ao IP | veja o motivo no log (`rota web recusada...`, `Tailscale-User-Login ... ignorado/fora da lista`) e configure a seção do proxy usado |
| `429` "muitas tentativas: aguarde" | 5 ou mais falhas seguidas do `/web/login` (contador global) | espere o `Retry-After` (de 1 min até 1 h); durante o bloqueio, nem o token certo entra. Um acerto zera. O desktop (`/ws`) e as sessões abertas seguem |
| `403` só pela Cloudflare; log `rota web recusada (Cloudflare Access): ...` | `JWT recusado: InvalidAudience` (AUD errado), `InvalidIssuer` (team domain errado), `ExpiredSignature` (relógio do hub ou sessão do Access vencida), `e-mail ... fora da lista`, `kid desconhecido` ou `chaves do Access em ...` (o hub não alcança `https://<team_domain>`) | corrija `[web.cloudflare]` ou o relógio (NTP); confira se o hub tem saída HTTPS |
| A PWA não instala (sem ícone ou sem "tela cheia") pela Cloudflare | sem cookies, o Access redirecionaria o `manifest.webmanifest` para o login; por isso o `<link rel="manifest">` leva `crossorigin="use-credentials"` | confira na aba de rede que o manifest volta `200` com JSON; se vier o HTML do login, registre na issue |
| O hub não sobe depois de mexer no config | chave desconhecida, `[web.cloudflare]` sem `team_domain`/`aud`/`emails`, `[web.tailscale]` sem `logins`, `[web.push] subject` sem `mailto:`/`https://` | a mensagem está no log do serviço |
| `404` em `/` com o proxy funcionando | `[web] enabled = false` (ou hub não reiniciado) | ligue e reinicie |
| O ambiente parou de enviar depois de trocar o `hub_url` para a URL HTTPS | o `ctowerd` só fala HTTP com o hub, e a Cloudflare fecha o `/sync/*` | volte o `hub_url` para `http://<ip>:7879` (§7) |
