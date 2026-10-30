# Ambiente (ctowerd) e runner no macOS

O `ctowerd` roda nativo no macOS como LaunchAgent (`packaging/launchd/ai.deep-ion.ctowerd.plist`), igual ao `cthubd`. `ctower restart`, `status` e `update` usam `launchctl` no lugar de `systemctl` (override: `CTOWER_SERVICE_MANAGER=systemd|launchd`). O atualizador roda num processo próprio (`setsid`), fora do serviço, em vez de uma unidade transitória do systemd.

## Instalar

```sh
scripts/provision/provision.sh --env      # node, claude, binários (da release; --from-source compila) e LaunchAgent
gh auth login && gh auth setup-git
claude                                     # login
```

Log: `~/Library/Logs/ctowerd.log`. Se `~/.cargo/bin/ctowerd` sumir, o LaunchAgent registra no log `ctowerd: executável ausente: <caminho>` e para de tentar (em vez de ficar em loop com exit 78), e o `ctower status` aponta o caminho que falta; restaure o binário (`provision.sh --env`) e rode `ctower restart`. Ligue ao hub remoto com `provision.sh --env --hub-url http://<ip>:7879` e o token de sync em `CT_HUB_TOKEN` (docs/hub.md, "Ambiente com hub remoto"); à mão, `hub_url` em `~/.config/ctower/config.toml` e `ctower hub set-token`.

## Disponível depois de ligar a máquina

LaunchAgent só sobe com o usuário logado, e o Claude Code lê a credencial do Keychain de login. Por isso a máquina precisa entrar sozinha na sessão depois de um boot ou queda de energia:

```sh
sudo pmset -a autorestart 1 sleep 0 disksleep 0 womp 1   # volta após queda de energia, sem suspender
sudo systemsetup -setremotelogin on                      # SSH
```

E em **Ajustes do Sistema → Usuários e Grupos → Login automático**, escolha o usuário (exige FileVault desligado). Para acesso fora da rede local, use a tailnet (Tailscale); o `ctowerd` escuta só em `127.0.0.1`, então acesse por túnel SSH (`ssh -L 7878:127.0.0.1:7878 <mac>`).

## Docs dos projetos (montagem do compartilhamento)

Os docs dos projetos ficam numa pasta da máquina do hub (`hub.local`), compartilhada pelo Samba como `ctdocs` (docs/hub.md, "Docs dos projetos"). O Mac monta o compartilhamento em `/Volumes/ctdocs` em todo login; o CT não monta nada, só usa o `docs_root`.

1. **Senha nas Chaves:** no Finder, Ir › Conectar ao Servidor (⌘K), `smb://ctdocs@hub.local/ctdocs`, a senha do usuário `ctdocs` do Samba (definida no `provision.sh --docs-samba` do hub) e marque **Lembrar esta senha nas Chaves**.
2. **Montagem no login:** o script `~/.local/bin/ctdocs-mount` (`chmod +x`) e o LaunchAgent `~/Library/LaunchAgents/ai.deep-ion.ctdocs-mount.plist` de docs/hub.md, "Montar no macOS", com `SEU_USUARIO` trocado pelo seu usuário; carregue com `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/ai.deep-ion.ctdocs-mount.plist`. Ele monta no login e confere a cada 5 min, remontando se a rede ou o hub caiu. Se o macOS perguntar se o `NetAuthAgent` pode usar a senha das Chaves, **Permitir sempre**. Log: `~/Library/Logs/ctdocs-mount.log`.
3. **Apontar o ambiente:** `docs_root = "/Volumes/ctdocs"` em `~/.config/ctower/config.toml` e `ctower restart`.
4. **Conferir:** `mount | grep ctdocs` mostra a montagem, `ls -a /Volumes/ctdocs` mostra o marcador `.ct-docs-root` e as pastas `<owner>__<repo>`, e o `env.info.docs` do ambiente vem com `reachable: true` (sem o selo "docs inacessíveis" na UI).

Como o LaunchAgent só roda com o usuário logado, a montagem depende do login automático da seção anterior. Desmontado, `/Volumes/ctdocs` não existe e as sessões sobem sem os docs, com o aviso `docs_unreachable`.

## Runner do GitHub Actions

O CI usa os runners hospedados do GitHub, então o Mac não precisa de runner. Para usar o seu como runner (só em repositório privado), veja "Runners próprios" em `docs/ci.md`.
