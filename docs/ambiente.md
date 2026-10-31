# Ambiente (`ctowerd`): configuração, logs, atualização e limpeza

Para quem instala e opera um **ambiente**: a máquina (distro WSL, Linux ou macOS) onde o `ctowerd` roda as sessões dos agentes. O hub, que guarda a configuração dos projetos e a visão consolidada, está em [`hub.md`](hub.md); o ambiente nativo no macOS, em [`ambiente-macos.md`](ambiente-macos.md).

## Instalação e configuração

`ctowerd` roda como serviço systemd de usuário na distro WSL (`scripts/provision/provision.sh`) e escuta só em `127.0.0.1`. O mesmo script instala o hub (`--hub`) ou os dois numa máquina só, já ligados (`--hub --env`); sem opção, instala só o ambiente. Detalhes em [`hub.md`](hub.md). Configuração opcional em `~/.config/ctower/config.toml`:

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
hub_url = "http://hub.example.com:7879"
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

# Log em arquivo, que a tela Logs da UI lê (veja "Logs" abaixo).
[logs]
dir = ""                            # vazio = ~/.local/state/ctower/logs (~/Library/Logs/ctower no macOS)
max_file_mb = 10                    # tamanho de cada arquivo antes de rotacionar; 0 desliga o arquivo
max_files = 5                       # arquivos guardados, contando o atual (ctowerd.jsonl, .1 … .4)
```

`update_ref` e `gh_sync_interval_secs` também podem ser editados pela UI (`env.config.set`): o daemon grava no `config.toml`, preservando o resto do arquivo, e aplica sem restart.

### Permissões pela UI

Cada sessão instala o hook `PermissionRequest` do Claude Code. Quando uma UI acompanha os pedidos (`session.permission.watch`), o pedido aparece nela (push `permission_request`) e o agente espera até 2 min pela resposta (`session.permission.respond`: permitir, sempre nesta sessão, negar ou responder no terminal). Sem resposta nesse prazo, sem UI acompanhando ou com o daemon fora do ar, o Claude Code mostra o menu de sempre no terminal.

### Sincronização com o GitHub

A cada ciclo o daemon espelha as issues abertas e as fechadas nos últimos 30 dias, as sub-issues dos épicos abertos e os PRs das branches dos work items, usando ETag. PR aberto leva o work item a `em_revisao`; PR mesclado, a `concluido`. Falhas mantêm o cache e aplicam backoff de até 15 min.

### Projetos Jira

O laço do GitHub pula os projetos com rastreador Jira: as issues deles vêm do [jira-cli](https://github.com/ankitpokhrel/jira-cli) do ambiente, sem rede própria do daemon. A cada ciclo o daemon roda o comando de credencial do cadastro (`/bin/sh -c`, sem TTY, prazo de 10 s, só a primeira linha) e passa a saída como `JIRA_API_TOKEN` só ao processo do `jira`; a senha não vai para log, evento nem mensagem de erro. A configuração do jira-cli é a do ambiente (`~/.config/.jira/.config.yml`, gerada por `jira init`, ou o `config_file` do cadastro como `JIRA_CONFIG_FILE`), com o `server` igual à URL do cadastro. O ciclo lista as issues alteradas desde o último sucesso (`updated >= -<m>m`), relê com `jira issue view --raw` só as novas ou alteradas e reconcilia uma vez por dia (abertas e fechadas nos últimos 30 dias). Categoria de status `done` fecha a issue. Com o cache do `gpg-agent` expirado, o comando de credencial falha ou trava, o sync fica em erro com o motivo e o backoff vale como no GitHub; rode o comando num terminal do ambiente e o ciclo seguinte volta. O `env.info` mostra a prontidão (`trackers`): jira-cli e versão, configuração, servidor, credencial, `jira me` e versão do Jira. Só o ambiente de sync do projeto envia o espelho ao hub.

### GitLab

O repositório dos projetos Jira fica no GitLab (self-hosted, HTTPS ou SSH). O clone é um `git clone` da URL do cadastro com a credencial do git do ambiente (credential helper, como o `osxkeychain`, ou a chave SSH), sem pedir senha (`GIT_TERMINAL_PROMPT=0`); se a credencial não responder a partir do daemon, o relato da atribuição sai com erro e o comando para clonar à mão. Um clone existente vale se o `origin` for o mesmo host e caminho, em HTTPS ou SSH. No mesmo ciclo do Jira, o daemon roda o comando de credencial do GitLab e chama o [glab](https://gitlab.com/gitlab-org/cli) com `GITLAB_HOST` do cadastro e `GITLAB_TOKEN` só no ambiente do processo, sem ler nem alterar a configuração global do glab: `glab version` e `glab api version` na prontidão (`env.info.trackers[].gitlab`) e `glab mr list -R <caminho> --all -s <branch> -F json` para cada work item em andamento. MR aberto leva a `em_revisao` e mesclado a `concluido`, como o PR; a limpeza apaga a branch remota no GitLab (`glab api -X DELETE`) só se a ponta for o último commit do MR. O guarda-corpo bloqueia sempre o `glab mr merge` e o merge pelo `glab api`.

## Diagnóstico

Diagnóstico: `ctower status`; reiniciar com sessões vivas exige `ctower restart --force`.

## Logs

O `ctowerd` e o `cthubd` escrevem o log em dois lugares. O stderr de sempre vai para o journald no Linux/WSL (`journalctl --user -u ctowerd`, `journalctl --user -u cthubd`) e para `~/Library/Logs/ctowerd.log` e `~/Library/Logs/cthubd.log` no macOS, pelo launchd. O segundo é um arquivo JSONL com rotação por tamanho, com uma linha por evento: `{id, ts, level, target, message, fields}`. No Linux/WSL ele fica em `~/.local/state/ctower/logs/ctowerd.jsonl` e `cthubd.jsonl` (`$XDG_STATE_HOME`). No macOS fica em `~/Library/Logs/ctower/`. Com `CTOWER_DATA_DIR` ou `CTHUB_DATA_DIR` definidos, vai para `<data>/logs`. O tamanho e a quantidade de arquivos vêm da tabela `[logs]` do `config.toml` de cada daemon. Tokens e secrets (tokens do GitHub e do GitLab, `Bearer …`, `token=…`, chaves age, JWT e os tokens de 64 hex do Control Tower) saem como `***` antes de gravar e de novo ao ler.

A UI mostra os três logs na tela **Manutenção › Logs**, com a mesma interface: o hub, cada ambiente e a própria UI. Há filtro de nível, busca, o modo **Seguir** (as linhas novas chegam pela conexão já aberta, sem polling), **Pausar**, **Quebrar linhas**, **Copiar** (o trecho selecionado ou as linhas da tela) e **Exportar** em `.log` ou `.jsonl`. No desktop, o arquivo exportado vai para a pasta Downloads. Os atalhos são **Logs do hub** na Visão geral do Hub, **Logs do ambiente** no detalhe do ambiente e `Logs: esta UI` na paleta. O protocolo tem `logs.tail { limit, before, level, q, since, until }`, `logs.follow` e `logs.unfollow`, com o push `logs.lines`. No hub, os mesmos métodos têm o prefixo `hub.`.

O log da UI guarda `console.error` e `console.warn`, os erros não tratados, as falhas de tela (o aviso "Esta tela falhou ao abrir") e as chamadas ao protocolo que falharam. No desktop ele fica também em `ui.jsonl`, na pasta de log do app (`~/Library/Logs/<id do app>` no macOS, `%LOCALAPPDATA%\<id do app>\logs` no Windows), e mostra a execução anterior. Na web/PWA o log da UI fica só em memória. Na web, o log de um ambiente vem pelo hub (`hub.env.call` com `logs.tail`): o botão **Atualizar** traz as linhas novas, e o modo seguir de um ambiente fica no app desktop. Um ambiente desconectado aparece como "Ambiente offline".

## Atualizar

Pela UI (**Configurações → Atualizações**) ou no terminal da distro:

```sh
ctower update --check      # versão instalada × a do update_ref
ctower update              # compila, instala e reinicia (recusa com sessões vivas)
ctower update --force      # idem, encerrando as sessões vivas (voltam como interrompidas)
ctower update --rollback   # volta para a versão anterior (pela UI: env.rollback)
```

Se o commit seguido é o da tag `vX.Y.Z` e a release tem o asset `ctower-env-vX.Y.Z-<arch>-<os>.tar.gz` (conferido pelo `.sha256`), o `ctower update` instala dele, sem Rust (precisa do `gh`). Sem asset, ou em outro commit, o build é local: um clone dedicado em `~/.local/share/ctower/update/src` (o clone de desenvolvimento não é tocado) segue o `update_ref`, e o `cargo install --locked` compila em `update/target`, com builds incrementais. A atualização roda numa unidade transitória do systemd (`ctower-update`), fora do serviço do daemon. Assim, o restart não a interrompe. Acompanhe com `journalctl --user -u ctower-update`. O andamento fica em `update/state.json` e o log em `update/update.log`.

Antes de instalar, o atualizador guarda os binários atuais em `update/prev/` e tira um snapshot do `ledger.db` (e do `hub.db` antigo, se ainda estiver no diretório do ambiente) (`VACUUM INTO`, em `update/db/`). Se o build falhar, nada muda. Se o daemon novo não responder com o commit novo em 60 s, os binários anteriores voltam, o banco também volta se o esquema tiver migrado (o banco em uso fica numa cópia `ledger.db.pre-rollback-<AAAAMMDD-HHMM>` ao lado), e o daemon reinicia. O protocolo do ambiente 2.0 tirou o papel de hub do `ctowerd`; a UI e o `ctowerd` precisam estar na mesma major.

## Limpeza do ambiente

Quando uma issue termina (work item `concluido`, PR mesclado ou fechado, ou issue fechada no espelho do GitHub), o ambiente pode limpar o que ela deixou, por etapas e nesta ordem: sessões → worktree → `target/` → branch local → branch remota (épico #284). Uma etapa só roda se a anterior terminou, e nada com mudança não commitada ou não enviada sai. O que fica aparece com o motivo.

O `ctowerd` monta o plano das limpezas pendentes no boot e a cada `check_interval_hours`, **sem apagar nada**; com `auto = true`, executa em seguida só as etapas pendentes, nunca as bloqueadas. O resumo (contagem e espaço) vai ao hub, que o mostra na tela Hub. O acionamento manual vale sempre, mesmo com o job desligado: na UI (**Manutenção → Limpeza**, por ambiente: "N limpezas pendentes · X GB", "Verificar agora" e "Limpar", tudo ou um item, e o que ficou fora do hub, com as regras de `[cleanup]` e um filtro por projeto, que o cartão **Limpeza** das Configurações do projeto abre já aplicado) ou no terminal:

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
