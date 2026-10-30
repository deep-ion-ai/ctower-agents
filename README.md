# Control Tower

Painel ("sala de controle") para acompanhar e orquestrar sessões longas e autônomas de agentes de código (Claude Code, Codex) em vários projetos, épicos e issues do GitHub, em várias máquinas.

Projeto de código aberto sob a licença [Apache-2.0](LICENSE). Contribuições são bem-vindas: veja [CONTRIBUTING.md](CONTRIBUTING.md). Vulnerabilidades: [SECURITY.md](SECURITY.md).

Status: MVP completo — daemon (`ctowerd`) com sessões, hooks, MCP do ledger, recuperação após queda e sincronização com o GitHub, e app de macOS (Tauri) em `ui/`. Spec em `docs/superpowers/specs/`, planos e roadmap em `docs/superpowers/plans/`.

## Daemon

`ctowerd` roda como serviço systemd de usuário na distro WSL (`scripts/provision/provision.sh`) e escuta só em `127.0.0.1`. O mesmo script instala o hub (`--hub`) ou os dois numa máquina só, já ligados (`--hub --env`); sem opção, instala só o ambiente. Detalhes em [`docs/hub.md`](docs/hub.md). Configuração opcional em `~/.config/ctower/config.toml`:

```toml
port = 7878
claude_bin = "claude"
ctower_bin = "ctower"
worktrees_root = "~/worktrees"
scrollback_bytes = 4194304
pre_trust_worktrees = true          # pré-aceita a confiança do Claude Code nas pastas das sessões

# Sincronização com o GitHub (somente leitura). Token: o configurado na UI
# (~/.config/ctower/github.token, 0600) ou, na falta dele, `gh auth token`
gh_sync_interval_secs = 180         # 0 desliga o laço; `sync.now` da UI continua funcionando
gh_api_url = "https://api.github.com"
gh_bin = "gh"

# Projetos Jira (issues lidas pelo jira-cli do ambiente; veja abaixo)
jira_sync_interval_secs = 300       # 0 desliga o laço; `sync.now` da UI continua funcionando
jira_bin = "jira"                   # nome no PATH ou caminho absoluto (/opt/homebrew/bin/jira)
jira_list_timeout_secs = 120        # prazo de cada página do `jira issue list` (a que falha é pedida de novo até 2 vezes)
jira_view_timeout_secs = 30         # prazo do `jira issue view` e das outras chamadas ao jira-cli
glab_bin = "glab"                   # GitLab dos projetos Jira (MRs, branch remota); /opt/homebrew/bin/glab

# Atualização do ctowerd/ctower (build local; veja "Atualizar" abaixo)
update_repo = "https://github.com/deep-ion-ai/ctower-agents.git"
update_ref = "main"                 # canal: branch (ex.: "develop") ou tag
update_check_interval_hours = 6     # 0 desliga a verificação periódica

# Hub (cthubd): origem dos projetos atribuídos a este ambiente (docs/hub.md).
# Obrigatório para executar: sem hub_url e hub.token o ambiente não tem projetos
# nem inicia sessões (retomada, hooks e MCP continuam).
hub_url = "http://mac-mini.tailnet:7879"
config_pull_interval_secs = 60      # pull da configuração; 0 desliga o laço
repos_root = "~/projetos"           # clone das atribuições sem repo_path
retention_days = 30                 # poda dos work items inativos já no hub; 0 desliga
secrets_max_age_hours = 24          # aviso de secrets de configuração velha no spawn; 0 desliga

# Limpeza após a conclusão do work item (veja "Limpeza do ambiente" abaixo).
# Tabela no fim do arquivo: as chaves acima ficam fora dela.
[cleanup]
check_interval_hours = 6            # job que monta o plano (também no boot); 0 desliga
auto = false                        # true: executa as etapas pendentes depois de cada verificação
min_age_hours = 24                  # horas desde a conclusão antes de limpar
min_idle_hours = 24                 # horas desde o último evento de sessão do work item
end_idle_sessions_hours = 0         # encerra a sessão ociosa de issue concluída após N h; 0 = só à mão
remove_agent_transcripts = true     # apaga os transcripts do Claude Code do worktree que saiu (com backup)
target_idle_days = 14               # apaga só o target/ de worktree que fica, inativo há N dias; 0 desliga
```

`update_ref` e `gh_sync_interval_secs` também podem ser editados pela UI (`env.config.set`): o daemon grava no `config.toml`, preservando o resto do arquivo, e aplica sem restart.

**Permissões pela UI.** Cada sessão instala o hook `PermissionRequest` do Claude Code. Quando uma UI acompanha os pedidos (`session.permission.watch`), o pedido aparece nela (push `permission_request`) e o agente espera até 2 min pela resposta (`session.permission.respond`: permitir, sempre nesta sessão, negar ou responder no terminal). Sem resposta nesse prazo, sem UI acompanhando ou com o daemon fora do ar, o Claude Code mostra o menu de sempre no terminal.

A cada ciclo o daemon espelha as issues abertas e as fechadas nos últimos 30 dias, as sub-issues dos épicos abertos e os PRs das branches dos work items, usando ETag. PR aberto leva o work item a `em_revisao`; PR mesclado, a `concluido`. Falhas mantêm o cache e aplicam backoff de até 15 min.

**Projetos Jira.** O laço do GitHub pula os projetos com rastreador Jira: as issues deles vêm do [jira-cli](https://github.com/ankitpokhrel/jira-cli) do ambiente, sem rede própria do daemon. A cada ciclo o daemon roda o comando de credencial do cadastro (`/bin/sh -c`, sem TTY, prazo de 10 s, só a primeira linha) e passa a saída como `JIRA_API_TOKEN` só ao processo do `jira`; a senha não vai para log, evento nem mensagem de erro. A configuração do jira-cli é a do ambiente (`~/.config/.jira/.config.yml`, gerada por `jira init`, ou o `config_file` do cadastro como `JIRA_CONFIG_FILE`), com o `server` igual à URL do cadastro. O ciclo lista as issues alteradas desde o último sucesso (`updated >= -<m>m`), relê com `jira issue view --raw` só as novas ou alteradas e reconcilia uma vez por dia (abertas e fechadas nos últimos 30 dias). Categoria de status `done` fecha a issue. Com o cache do `gpg-agent` expirado, o comando de credencial falha ou trava, o sync fica em erro com o motivo e o backoff vale como no GitHub; rode o comando num terminal do ambiente e o ciclo seguinte volta. O `env.info` mostra a prontidão (`trackers`): jira-cli e versão, configuração, servidor, credencial, `jira me` e versão do Jira. Só o ambiente de sync do projeto envia o espelho ao hub.

**GitLab.** O repositório dos projetos Jira fica no GitLab (self-hosted, HTTPS ou SSH). O clone é um `git clone` da URL do cadastro com a credencial do git do ambiente (credential helper, como o `osxkeychain`, ou a chave SSH), sem pedir senha (`GIT_TERMINAL_PROMPT=0`); se a credencial não responder a partir do daemon, o relato da atribuição sai com erro e o comando para clonar à mão. Um clone existente vale se o `origin` for o mesmo host e caminho, em HTTPS ou SSH. No mesmo ciclo do Jira, o daemon roda o comando de credencial do GitLab e chama o [glab](https://gitlab.com/gitlab-org/cli) com `GITLAB_HOST` do cadastro e `GITLAB_TOKEN` só no ambiente do processo, sem ler nem alterar a configuração global do glab: `glab version` e `glab api version` na prontidão (`env.info.trackers[].gitlab`) e `glab mr list -R <caminho> --all -s <branch> -F json` para cada work item em andamento. MR aberto leva a `em_revisao` e mesclado a `concluido`, como o PR; a limpeza apaga a branch remota no GitLab (`glab api -X DELETE`) só se a ponta for o último commit do MR. O guarda-corpo bloqueia sempre o `glab mr merge` e o merge pelo `glab api`.

### Ledger centralizado (várias máquinas)

O **hub** é um binário próprio, o `cthubd` (porta 7879, config em `~/.config/cthub/`, dados em `~/.local/share/cthub/`): recebe o que cada ambiente produz e guarda a visão consolidada em `hub.db`, com snapshots periódicos. Ele não roda sessões. Projetos são cadastrados e atribuídos aos ambientes só no hub; o ambiente não cria nem edita projetos. Cada ambiente (`ctowerd`) se liga ao hub com `hub_url` e um token de sync (`cthubd add-env <nome>` no hub, `ctower hub set-token` no ambiente), puxa a configuração dos projetos atribuídos e envia o que produz. O `ctowerd` com `hub = true` no config (o hub antigo) recusa subir: migre com `cthubd migrate --from ~/.local/share/ctower`, pelo roteiro da seção 5 de `docs/hub.md`. Configuração, backup e roteiro de reconstrução: [`docs/hub.md`](docs/hub.md). A UI web (PWA) no celular, por `tailscale serve` ou Cloudflare Tunnel com Access: [`docs/acesso-remoto.md`](docs/acesso-remoto.md).

O hub se atualiza pela UI, em **Manutenção › Atualizações › Hub** (#421), ou com `cthubd update` (`--check`, `--rollback`) no terminal da máquina do hub, com o mesmo desenho do `ctower update` abaixo: build local do `update_ref` do config do hub, snapshot do `hub.db` e rollback se o hub novo não responder `/healthz` e `hub.info`. Pela UI, o atualizador roda fora do serviço (unidade `cthubd-update` no systemd; processo próprio no macOS), o andamento aparece ao vivo e a UI reconecta depois do restart. Atualize o hub antes dos ambientes. Antes de voltar o `hub.db`, o rollback guarda o banco em uso numa cópia `pre-rollback`; o rollback manual pede `--force` no terminal ou uma segunda confirmação na UI (docs/hub.md, "Atualizar o hub").

Diagnóstico: `ctower status`; reiniciar com sessões vivas exige `ctower restart --force`.

### Atualizar

Pela UI (**Configurações → Atualizações**) ou no terminal da distro:

```sh
ctower update --check      # versão instalada × a do update_ref
ctower update              # compila, instala e reinicia (recusa com sessões vivas)
ctower update --force      # idem, encerrando as sessões vivas (voltam como interrompidas)
ctower update --rollback   # volta para a versão anterior (pela UI: env.rollback)
```

Se o commit seguido é o da tag `vX.Y.Z` e a release tem o asset `ctower-env-vX.Y.Z-<arch>-<os>.tar.gz` (conferido pelo `.sha256`), o `ctower update` instala dele, sem Rust (precisa do `gh`). Sem asset, ou em outro commit, o build é local: um clone dedicado em `~/.local/share/ctower/update/src` (o clone de desenvolvimento não é tocado) segue o `update_ref`, e o `cargo install --locked` compila em `update/target`, com builds incrementais. A atualização roda numa unidade transitória do systemd (`ctower-update`), fora do serviço do daemon. Assim, o restart não a interrompe. Acompanhe com `journalctl --user -u ctower-update`. O andamento fica em `update/state.json` e o log em `update/update.log`.

Antes de instalar, o atualizador guarda os binários atuais em `update/prev/` e tira um snapshot do `ledger.db` (e do `hub.db` antigo, se ainda estiver no diretório do ambiente) (`VACUUM INTO`, em `update/db/`). Se o build falhar, nada muda. Se o daemon novo não responder com o commit novo em 60 s, os binários anteriores voltam, o banco também volta se o esquema tiver migrado (o banco em uso fica numa cópia `ledger.db.pre-rollback-<AAAAMMDD-HHMM>` ao lado), e o daemon reinicia. O protocolo do ambiente 2.0 tirou o papel de hub do `ctowerd`; a UI e o `ctowerd` precisam estar na mesma major.

### Limpeza do ambiente

Quando uma issue termina (work item `concluido`, PR mesclado ou fechado, ou issue fechada no espelho do GitHub), o ambiente pode limpar o que ela deixou, por etapas e nesta ordem: sessões → worktree → `target/` → branch local → branch remota (épico #284). Uma etapa só roda se a anterior terminou, e nada com mudança não commitada ou não enviada sai. O que fica aparece com o motivo.

O `ctowerd` monta o plano das limpezas pendentes no boot e a cada `check_interval_hours`, **sem apagar nada**; com `auto = true`, executa em seguida só as etapas pendentes, nunca as bloqueadas. O resumo (contagem e espaço) vai ao hub, que o mostra na tela Hub. O acionamento manual vale sempre, mesmo com o job desligado: na UI (**Manutenção → Limpeza**, por ambiente: "N limpezas pendentes · X GB", "Verificar agora" e "Limpar", tudo ou um item, e o que ficou fora do hub; as regras de `[cleanup]` ficam em **Configurações → Ambiente → Limpeza**) ou no terminal:

```sh
ctower clean --dry-run          # o plano: pendentes, bloqueadas com o motivo e o espaço
ctower clean                    # executa as etapas pendentes
ctower clean --item 251         # só o work item da issue #251 (--repo owner/repo se ambíguo)
```

Cada execução vai para o log do daemon e para `~/.local/share/ctower/cleanup/runs.jsonl` (o que saiu, o que ficou e por quê, e o espaço liberado).

- **Worktree** (#265): o worktree do work item sai com `git worktree remove` (sem `--force`) e `git worktree prune`, levando o `target/` e os transcripts do Claude Code com backup. Fica, com o motivo, se houver sessão aberta nele, mudança não commitada (inclusive arquivo novo fora do `.gitignore`), commit não enviado (à frente do upstream, ou fora de qualquer branch remota: sem upstream, upstream apagado ou HEAD destacado), worktree travado, ou se o caminho for o clone principal ou a pasta do gestor.
- **`target/`** (#265): de um worktree que fica, sai só o `target/` (o código fica), depois de `target_idle_days` dias sem atividade e sem sessão aberta. Contam como atividade as sessões no worktree, o índice do git e as pastas do próprio `target/` (um `cargo build` à mão conta). Vale para o worktree de issue concluída que não pode sair e para o de issue ainda em andamento: este entra no plano só com a etapa `target/`, como "em andamento, worktree inativo". O `target/` precisa estar no `.gitignore` e sem arquivo versionado.
- **Branch local** (#286): depois que o worktree sai, a branch do work item sai do clone principal quando o último PR foi mesclado e a ponta local é o head do PR, ou está contida nele (cobre o squash merge, que o `git branch -d` recusa), ou quando o PR foi fechado sem merge (ou não houve PR) e a branch não tem commit fora do remoto. A remoção só vale para o SHA avaliado (`git update-ref -d`), então um commit feito entre o plano e a execução segura a branch.
- **Branch remota** (#286): `origin/<branch>` sai pela API do GitHub, com o token do daemon (`github.token` ou `gh auth token`), quando o último PR foi mesclado ou fechado e a ponta remota é o head do PR (ninguém empurrou depois). O plano compara a referência remota local; a execução confere a ponta no GitHub na hora, apaga e roda `git fetch --prune origin`. Sem PR, ou com o sync do GitHub desativado, a remota fica. O head do PR vem do espelho do GitHub (`pr_cache.head_sha`, da mesma listagem com ETag do sync); o PR gravado antes dessa coluna é buscado uma vez na API.
- **Nunca saem**, nem a local nem a remota: `develop`, `main`, `master`, a base e as branches protegidas do projeto, `release-please--*`, a branch que é head ou base de um PR aberto e a de épico (`wip/<n>-…`) com sub-issue aberta. O que fica aparece como "`<branch>` não removida: motivo" (commit depois do head do PR, commit fora do remoto, push depois do PR etc.).

## App de macOS (`ui/`)

Tauri 2 + React. A conexão com os daemons fica no lado Rust (crate `ct-client`): o token de UI nunca chega ao JavaScript. O app é compilado só para macOS (Apple Silicon).

- **Instalação:** `scripts/install-app.sh` (ou, sem clone, `gh api repos/deep-ion-ai/ctower-agents/contents/scripts/install-app.sh -H 'Accept: application/vnd.github.raw' | bash`). Baixa o `control-tower_X.Y.Z_aarch64.dmg` da release mais recente pelo `gh` e instala o app em `/Applications`, sem a quarentena: o app não é notarizado, e baixado pelo navegador o Gatekeeper o bloqueia na primeira abertura. Opções em `--help` (`--version`, `--dir`, `--open`). Pelo navegador, arraste o app para Aplicativos e libere-o em Ajustes do Sistema → Privacidade e Segurança (ou `xattr -dr com.apple.quarantine "/Applications/Control Tower.app"`). Publicado em cada release pelo workflow `ui` (job `macos`), chamado pelo `release`; veja [Publicar uma versão](#publicar-uma-versão). **Não há mais app de Windows**: quem o usa deixa de receber atualizações.
- **Atualização do app:** o app procura versão nova ao abrir e a cada 6 h (`tauri-plugin-updater`). Mostra "App X disponível" no cabeçalho e instala em **Configurações → Atualizações → Atualizar o app**, reiniciando a janela; as sessões continuam nos daemons. O repositório é privado: o app lê as releases com o token do GitHub (`github.token` ou `gh auth token`), em Rust, sem passá-lo ao JavaScript.
- **App de teste de uma branch:** `gh workflow run ui --ref <branch> -f preview=true` gera o `.dmg` como artefato do run (`docs/ci.md`).
- **Desenvolvimento do frontend:** `cd ui && npm ci && npm test` (vitest) e `npm run typecheck`. Os tipos do protocolo do ambiente em `ui/src/protocol/gen` são gerados com `cargo test -p ct-server --features ts export_ts`, e os do `/ws` do hub em `ui/src/protocol/hub-gen` com `cargo test -p cthubd --features ts export_ts`; o CI falha se estiverem desatualizados.
- **Lado Rust do app:** `cargo test --manifest-path ui/src-tauri/Cargo.toml` (fora do workspace; no Linux precisa de `libwebkit2gtk-4.1-dev`).
- **Roteiro de teste manual:** `docs/ui-roteiro-manual.md`.

### Publicar uma versão

A versão segue SemVer e sai dos commits ([Conventional Commits](https://www.conventionalcommits.org/pt-br/)), pelo [release-please](https://github.com/googleapis/release-please). Ninguém sobe versão nem cria tag à mão.

1. **`develop → main`:** o merge na `main` dispara o workflow `release`, que abre ou atualiza o PR **"chore(main): release X.Y.Z"**. O PR sobe a versão em `Cargo.toml` (workspace), `ui/package.json`, `ui/src-tauri/Cargo.toml` e `ui/src-tauri/tauri.conf.json`, atualiza o `CHANGELOG.md` e, num commit à parte, os três lockfiles. A cada merge novo na `main`, o mesmo PR é refeito.
2. **Mesclar o PR de release** é o botão: nem todo merge na `main` vira release. Ao mesclar, o release-please cria a tag `vX.Y.Z` e a release, ainda em rascunho.
3. **Publicação:** o job `cthubd` do `release` anexa o binário do hub para Linux (`cthubd-vX.Y.Z-<arch>-linux.tar.gz`, com a unidade systemd, e o `.sha256`). Depois, o `release` chama o workflow `ui`, que compila no macOS e envia o `.dmg`, o `.app.tar.gz` assinado (+ `.sig`) e o `latest.json` (`darwin-aarch64`), e só então tira a release do rascunho. Até ali, o updater do app continua vendo a versão anterior. O `ctowerd`, o `ctower` e o `cthubd` têm a versão do workspace. Os atualizadores (`ctower update`, `cthubd update`) compilam do fonte e não usam os binários da release. Não há binário do hub para macOS.
4. **Back-merge:** o `release` abre o PR **"chore: back-merge vX.Y.Z"** (`main → develop`) com o commit de release. Mescle com merge commit (não squash), para o próximo `develop → main` não dar conflito em versão nem no `CHANGELOG.md`.

Enquanto a versão for `0.x` (`bump-minor-pre-major`):

| Commit | Exemplo | Versão |
| --- | --- | --- |
| `fix:`, `perf:`, `revert:` | `fix(ui): reconecta depois de suspender` | patch (0.3.0 → 0.3.1) |
| `feat:` | `feat(ct-gh): sincroniza milestones` | minor (0.3.1 → 0.4.0) |
| `!` depois do tipo ou rodapé `BREAKING CHANGE:` (em qualquer tipo) | `feat(ct-server)!: remove o método session.peek` | minor (0.4.0 → 0.5.0); a partir da 1.0, major |
| `docs:`, `chore:`, `ci:`, `test:`, `refactor:`, `build:`, `style:`, merge e tipos fora da convenção | `docs: atualiza o README` | nenhuma: sozinhos, não geram release |

- **Forçar uma versão:** um commit com `Release-As: X.Y.Z` no corpo (ex.: `git commit --allow-empty -m "chore: release 1.0.0" -m "Release-As: 1.0.0"`), que chega à `main` pelo fluxo normal. O PR de release passa a propor essa versão.
- **Binário do cthubd que falhou:** o `publish` não roda e a release fica em rascunho. Reexecute os jobs que falharam no run do `release` (**Re-run failed jobs**).
- **Publicação que falhou:** a tag e a release em rascunho já existem. Rode **Actions → ui → Run workflow** com `tag: vX.Y.Z`. O workflow recompila, reenvia os arquivos (`--clobber`) e publica a release. Não crie nem empurre tag à mão: a tag tem de apontar para o commit de release.
- **Versões divergentes:** `python3 scripts/check-versions.py` confere os arquivos de versão e os lockfiles; o CI roda o mesmo check.
- **Configuração:** `release-please-config.json` e `.release-please-manifest.json` (última versão publicada; o release-please o atualiza no PR de release). O repositório precisa de **Settings → Actions → General → Allow GitHub Actions to create and approve pull requests**. O PR de release é aberto pelo `GITHUB_TOKEN`, e os runs de `ci` e `ui` nele ficam em **action_required**, à espera de aprovação. Eles não são exigidos: a versão, o `CHANGELOG.md` e os lockfiles são conferidos pelo check de versões dentro do job `lockfiles` do `release`, e a `main` não tem checks obrigatórios. Mescle o PR de release sem aprovar os runs. Se quiser os checks completos, aprove-os na aba **Checks** do PR (issue #159).
- **Assinatura do updater (sem notarização):** usa os secrets `TAURI_SIGNING_PRIVATE_KEY` e `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, e a chave pública fica em `plugins.updater.pubkey`. Para trocar a chave, gere um par novo com `npx tauri signer generate -w ~/.tauri/control-tower-updater.key`, atualize a pubkey e os secrets e reinstale o app à mão uma vez.
