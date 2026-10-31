# App desktop (`ui/`)

Para quem usa o app desktop do Control Tower: como instalar no macOS, onde ele é distribuído, e como compilar no Linux e no Windows, onde não é.

O app é Tauri 2 + React. A conexão com os daemons fica no lado Rust (crate `ct-client`): o token de UI nunca chega ao JavaScript. Ele conversa com os ambientes (`ctowerd`) e com o hub (`cthubd`); a configuração de cada um está em [`ambiente.md`](ambiente.md) e [`hub.md`](hub.md).

| Sistema | Situação | Atualização |
| --- | --- | --- |
| macOS (Apple Silicon) | distribuído em cada release | automática, pelo app |
| Linux | compile por conta própria | compilar de novo |
| Windows | compile por conta própria | compilar de novo |

Fora do macOS, o caminho principal é a **UI web (PWA)** que o hub serve: abre no navegador do computador ou do celular, sem compilar nada ([`acesso-remoto.md`](acesso-remoto.md) e [`hub.md`](hub.md), seção "UI web").

## macOS (distribuído)

O app é compilado só para macOS (Apple Silicon), publicado em cada release pelo workflow `ui` (job `macos`), chamado pelo `release` ([`release.md`](release.md)).

- **Instalação:** `scripts/install-app.sh` (ou, sem clone, `gh api repos/deep-ion-ai/ctower-agents/contents/scripts/install-app.sh -H 'Accept: application/vnd.github.raw' | bash`). Baixa o `control-tower_X.Y.Z_aarch64.dmg` da release mais recente pelo `gh` e instala o app em `/Applications`, sem a quarentena: o app não é notarizado, e baixado pelo navegador o Gatekeeper o bloqueia na primeira abertura. Opções em `--help` (`--version`, `--dir`, `--open`). Pelo navegador, arraste o app para Aplicativos e libere-o em Ajustes do Sistema → Privacidade e Segurança (ou `xattr -dr com.apple.quarantine "/Applications/Control Tower.app"`).
- **Atualização do app:** o app procura versão nova ao abrir e a cada 6 h (`tauri-plugin-updater`). Mostra "App X disponível" no cabeçalho e instala em **Configurações → Atualizações → Atualizar o app**, reiniciando a janela; as sessões continuam nos daemons. O app lê as releases com o token do GitHub (`github.token` ou `gh auth token`), em Rust, sem passá-lo ao JavaScript.
- **App de teste de uma branch:** `gh workflow run ui --ref <branch> -f preview=true` gera o `.dmg` como artefato do run ([`ci.md`](ci.md)).
- **Roteiro de teste manual:** [`ui-roteiro-manual.md`](ui-roteiro-manual.md).

## Linux (compilar por conta própria)

Não há pacote publicado. O roteiro abaixo foi verificado no Ubuntu 22.04 (o mesmo sistema do runner do CI), gerando e instalando o `.deb` e gerando o AppImage.

### Pré-requisitos

- **Rust** estável, pelo [rustup](https://rustup.rs/) (o `rust-toolchain.toml` da raiz escolhe o canal).
- **Node 22** com npm.
- As bibliotecas do WebKitGTK e as demais que a [documentação do Tauri 2](https://v2.tauri.app/start/prerequisites/#linux) lista. No Debian e no Ubuntu:

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Em outras distribuições (Fedora, Arch, openSUSE), use os pacotes equivalentes da mesma página.

### Compilar

```sh
git clone https://github.com/deep-ion-ai/ctower-agents.git
cd ctower-agents/ui
npm ci
npx tauri build --bundles deb,appimage
```

O build em modo release leva vários minutos na primeira vez. Os pacotes ficam em `ui/src-tauri/target/release/bundle/`:

- `deb/Control Tower_X.Y.Z_amd64.deb`
- `appimage/Control Tower_X.Y.Z_amd64.AppImage`

(`aarch64` no lugar de `amd64` numa máquina ARM.) Para gerar só um dos formatos, passe só ele em `--bundles`; `rpm` também é aceito.

### Instalar

- **`.deb`:** `sudo apt install "./src-tauri/target/release/bundle/deb/Control Tower_X.Y.Z_amd64.deb"`. O executável fica em `/usr/bin/control-tower`, com o atalho "Control Tower" no menu de aplicativos. Para remover: `sudo apt remove control-tower`.
- **AppImage:** `chmod +x "Control Tower_X.Y.Z_amd64.AppImage"` e abra o arquivo. Ele não instala nada; copie-o para onde quiser (por exemplo, `~/Applications`).

### Ligar aos ambientes

No Linux não há WSL: o ambiente padrão da lista ("control-tower (WSL)") não abre. Em **Configurações › Conexões**, adicione o ambiente com o tipo **Remoto (token informado aqui)**:

- **URL:** `ws://127.0.0.1:7878/ws` para o `ctowerd` desta máquina, ou `ws://<máquina>:7878/ws` por um túnel SSH (`ssh -L 7878:127.0.0.1:7878 <máquina>`), porque o `ctowerd` só escuta em `127.0.0.1`;
- **Token de UI:** o conteúdo de `~/.config/ctower/ui.token` na máquina do ambiente.

O hub entra em **Configurações → Hub**, como em qualquer sistema ([`hub.md`](hub.md), "Ligar a UI ao hub").

## Windows (compilar por conta própria)

Não há instalador publicado. **Este roteiro segue a documentação do Tauri 2 e foi verificado só quanto ao build** (o instalador NSIS é gerado no runner `windows-latest` do GitHub Actions). Ninguém o testou ainda numa máquina Windows, abrindo o app e ligando-o a um ambiente na WSL. Se você testar, conte numa issue.

### Pré-requisitos

Pela [documentação do Tauri 2](https://v2.tauri.app/start/prerequisites/#windows):

- **Microsoft C++ Build Tools** (Visual Studio Build Tools), com a carga de trabalho "Desenvolvimento para desktop com C++".
- **WebView2:** já vem no Windows 10 (versão 1803 ou mais nova) e no Windows 11. Se faltar, instale o "Evergreen Bootstrapper" da Microsoft.
- **Rust** pelo [rustup](https://rustup.rs/), com a toolchain MSVC (`rustup default stable-msvc`).
- **Node 22** com npm.

### Gerar o `icon.ico`

O repositório não tem o `icon.ico`, e o build do Windows precisa dele. Gere os ícones numa pasta à parte, a partir do `icon.png`, e copie só o `.ico`, para não sobrescrever os ícones do macOS:

```powershell
cd ctower-agents\ui
npm ci
npx tauri icon src-tauri\icons\icon.png -o $env:TEMP\ct-icons
Copy-Item $env:TEMP\ct-icons\icon.ico src-tauri\icons\icon.ico
```

### Compilar

```powershell
npx tauri build --bundles nsis
```

O instalador fica em `ui\src-tauri\target\release\bundle\nsis\Control Tower_X.Y.Z_x64-setup.exe`. Para um `.msi`, use `--bundles msi` (o WiX é baixado pelo Tauri na primeira vez). Por padrão, o instalador NSIS do Tauri instala por usuário, sem pedir administrador.

### Ligar ao ambiente na WSL

O app já trata o ambiente na WSL, que é o padrão da lista ("control-tower (WSL)", URL `ws://127.0.0.1:7878/ws`, distro `control-tower`):

- ao abrir, o app sobe a distro (`wsl -d <distro> --exec true`, sem janela de console) e lê o token de UI de `~/.config/ctower/ui.token` dentro dela; o token nunca é gravado no Windows;
- **Abrir pasta de docs** abre o caminho da distro pelo Explorer (`\\wsl.localhost\<distro>\...`);
- o log do próprio app fica em `%LOCALAPPDATA%\<id do app>\logs` ([`ambiente.md`](ambiente.md), "Logs").

Para outra distro, edite o ambiente em **Configurações › Conexões** (tipo **WSL**, campo **Distro**). Um ambiente em outra máquina entra como **Remoto**, como no Linux.

## Limitações fora do macOS

- **Sem binário publicado e sem atualização automática:** o updater só publica artefatos de macOS. Para atualizar, faça `git pull` (ou troque de tag) e compile de novo. A verificação de versão nova do app, em **Configurações → Atualizações**, termina em erro nesses sistemas, porque o `latest.json` da release só traz o macOS (`darwin-aarch64`).
- **Sem suporte oficial:** o CI compila e testa o lado Rust do app no Linux, mas não empacota nem testa o app em uso no Linux ou no Windows. Problemas específicos desses sistemas podem demorar a ser resolvidos.
- **Caminho principal:** a PWA servida pelo hub ([`acesso-remoto.md`](acesso-remoto.md)), que funciona em qualquer navegador e se atualiza junto com o hub.

Antes, havia um app de Windows publicado; ele deixou de ser publicado, e quem usava aquela versão não recebe mais atualizações.
