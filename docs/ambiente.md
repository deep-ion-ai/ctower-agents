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
test_mode = false                   # ambiente de teste: a UI o marca (docker/test, #176)

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

# Comandos de credencial do Jira e do GitLab só deste ambiente, no lugar dos
# do cadastro (veja "Credencial por ambiente" abaixo). Uma tabela por projeto.
[credentials."acme/acme-web"]
tracker_cmd = "/usr/bin/pass show acme/jira | head -1"     # vazio ou ausente = o do cadastro
forge_cmd = "/usr/bin/pass show acme/gitlab | head -1"     # idem, para o GitLab
```

`update_ref` e `gh_sync_interval_secs` também podem ser editados pela UI (`env.config.set`): o daemon grava no `config.toml`, preservando o resto do arquivo, e aplica sem restart. O mesmo vale para `[credentials]`, em Configurações › Ambiente › Credenciais do Jira e GitLab (`env.credentials.get|set`).

### Permissões pela UI

Cada sessão instala o hook `PermissionRequest` do Claude Code. Quando uma UI acompanha os pedidos (`session.permission.watch`), o pedido aparece nela (push `permission_request`) e o agente espera até 2 min pela resposta (`session.permission.respond`: permitir, sempre nesta sessão, negar ou responder no terminal). Sem resposta nesse prazo, sem UI acompanhando ou com o daemon fora do ar, o Claude Code mostra o menu de sempre no terminal.

### Sincronização com o GitHub

A cada ciclo o daemon espelha as issues abertas e as fechadas nos últimos 30 dias, as sub-issues dos épicos abertos e os PRs das branches dos work items, usando ETag. PR aberto leva o work item a `em_revisao`; PR mesclado, a `concluido`. Falhas mantêm o cache e aplicam backoff de até 15 min.

### Projetos Jira

O laço do GitHub pula os projetos com rastreador Jira: as issues deles vêm do [jira-cli](https://github.com/ankitpokhrel/jira-cli) do ambiente, sem rede própria do daemon. A cada ciclo o daemon roda o comando de credencial do cadastro (`/bin/sh -c`, sem TTY, prazo de 10 s, só a primeira linha) e passa a saída como `JIRA_API_TOKEN` só ao processo do `jira`; a senha não vai para log, evento nem mensagem de erro. A configuração do jira-cli é a do ambiente (`~/.config/.jira/.config.yml`, gerada por `jira init`, ou o `config_file` do cadastro como `JIRA_CONFIG_FILE`), com o `server` igual à URL do cadastro. O ciclo lista as issues alteradas desde o último sucesso (`updated >= -<m>m`), relê com `jira issue view --raw` só as novas ou alteradas e reconcilia uma vez por dia (abertas e fechadas nos últimos 30 dias). Categoria de status `done` fecha a issue. Com o cache do `gpg-agent` expirado, o comando de credencial falha ou trava, o sync fica em erro com o motivo e o backoff vale como no GitHub; rode o comando num terminal do ambiente e o ciclo seguinte volta. O `env.info` mostra a prontidão (`trackers`): jira-cli e versão, configuração, servidor, credencial, `jira me` e versão do Jira. Só o ambiente de sync do projeto envia o espelho ao hub.

### Responsáveis e o "eu" do rastreador

Cada issue do espelho guarda os identificadores dos responsáveis (`assignees`): no GitHub, o `login` de cada um; no Jira, os do `fields.assignee` que vierem (`name` e `key` no Server/DC, `accountId` e `emailAddress` no Cloud). O "eu" do projeto fica no `sync_state` (`me`): no GitHub, o login do token que o sync usa (o do daemon ou, no hub, o do hub); no Jira, a saída do `jira me` do ambiente de sync, que também vai na prontidão (`jira.me`) e chega ao hub pelo `tracker_status`. O `jira me` não fala com o servidor: imprime o `login` do `.config.yml` gravado pelo `jira init`, que é o username no Server/DC e o e-mail no Cloud. A comparação não diferencia maiúsculas e aceita qualquer um dos identificadores; no Cloud, ela depende de o Jira mandar o `emailAddress` do responsável (a privacidade da conta pode escondê-lo).

### Credencial por ambiente

O comando de credencial do Jira e do GitLab vem do cadastro do projeto no hub e é o mesmo para todos os ambientes. Quando o caminho do binário ou o gerenciador de senhas muda de um ambiente para outro (por exemplo, `/opt/homebrew/bin/pass` no macOS e `/usr/bin/pass` no Linux), cada ambiente pode usar o seu próprio comando em `[credentials."owner/repo"]` (`tracker_cmd` para o Jira, `forge_cmd` para o GitLab). Também dá para editar pela UI, em Configurações › Ambiente › Credenciais do Jira e GitLab, que mostra o comando do cadastro, o deste ambiente e o resultado da última leitura (credencial ok ou o erro, e o sync falhando há N ciclos). O comando deste ambiente vale no sync, no "Ver issue", no glab e no prompt da sessão, onde substitui o `"$(<comando>)"` do cadastro. A mudança vale no próximo uso, sem reiniciar. Os outros ambientes e o cadastro do hub continuam iguais, e o comando nunca é enviado ao hub. Na prontidão (`env.info.trackers`), `credential_from_env` indica que a credencial veio deste ambiente.

### Épico da sessão de issue

As regras do épico chegam à sessão de uma issue pelo espelho do rastreador (`issue_cache`), que liga a issue ao épico. Num projeto Jira, se a issue ainda não está no espelho (sync falhando, issue fora do JQL), o `session.start` lê a issue direto no Jira com `jira issue view` e a grava no cache antes de criar a sessão. Se mesmo assim não der para resolver o épico, a sessão sobe com um aviso no system prompt para o agente parar e perguntar antes de trabalhar. Também saem o alerta `epic_unresolved` na UI e um evento no work item, e `ledger_get_context` e `rules_get` trazem `epic_resolution: "falhou: <motivo>"`, com o último erro do sync quando ele está falhando. Com o épico resolvido, `epic_resolution` vem como `"ok"`.

Para trabalhar numa issue do Jira fora do JQL, `tracker.add_issue { project_id | owner+repo, key }` a traz ao espelho deste ambiente pelo ID (`ACME-123`, `123` ou o link da issue), com os pais dela (o épico, pelo Epic Link ou pelo `parent`). Pelo hub, `hub.tracker.add_issue` chama o mesmo método no ambiente de sync do projeto. Já no espelho, a resposta vem sem chamar o Jira. No ambiente de sync, as issues gravadas sobem ao hub como `issue_mirror` e o outbox é enviado antes da resposta. Sem work item, a issue sai na próxima reconciliação com poda; com work item, fica (`jira:pinned`). Cada erro tem a sua mensagem: chave inválida ou de outro projeto, issue que não existe no Jira, tipo ignorado pelo mapa de tipos, e credencial que falhou ou senha recusada.

### Rótulo de aprovação

Com o rótulo de aprovação exigido no cadastro do projeto no hub (`approval: { label, required: true }`, #124, veja o [hub](hub.md)), o `session.start` de uma issue sem o rótulo é recusado pelo ambiente antes de criar work item, branch ou worktree, com o erro `APPROVAL_REQUIRED` (-32052) e a mensagem `aguardando aprovação: <item> não tem o rótulo "<rótulo>", que o projeto exige para iniciar sessão`. Os rótulos são os do espelho (`issue_cache.labels`, do GitHub ou do Jira); com a issue fora do espelho, valem os `labels` do pedido. O épico conta pelo próprio rótulo e a sub-issue pelo dela; `ledger_start_sub_issue` também recusa a sub-issue sem o rótulo. `override_approval: true` ("Iniciar mesmo assim", que só a UI manda depois da confirmação) inicia mesmo assim e grava a decisão `orientada` "Sessão iniciada sem o rótulo de aprovação" no work item, ligada à sessão. A sessão autônoma (`autonomous: true`, #181) passa pela mesma regra, antes de alinhar a branch de integração. `session.start_project` (planejamento) e `session.start_gestor` não passam pela regra. Projeto local legado e hub anterior: o padrão, sem exigir.

### GitLab

O repositório dos projetos Jira fica no GitLab (self-hosted, HTTPS ou SSH). O clone é um `git clone` da URL do cadastro com a credencial do git do ambiente (credential helper, como o `osxkeychain`, ou a chave SSH), sem pedir senha (`GIT_TERMINAL_PROMPT=0`); se a credencial não responder a partir do daemon, o relato da atribuição sai com erro e o comando para clonar à mão. Um clone existente vale se o `origin` for o mesmo host e caminho, em HTTPS ou SSH. No mesmo ciclo do Jira, o daemon roda o comando de credencial do GitLab e chama o [glab](https://gitlab.com/gitlab-org/cli) com `GITLAB_HOST` do cadastro e `GITLAB_TOKEN` só no ambiente do processo, sem ler nem alterar a configuração global do glab: `glab version` e `glab api version` na prontidão (`env.info.trackers[].gitlab`) e `glab mr list -R <caminho> --all -s <branch> -F json` para cada work item em andamento. MR aberto leva a `em_revisao` e mesclado a `concluido`, como o PR; a limpeza apaga a branch remota no GitLab (`glab api -X DELETE`) só se a ponta for o último commit do MR. O guarda-corpo bloqueia sempre o `glab mr merge` e o merge pelo `glab api`.

### Modo autônomo (branch `wip/autonoma`)

Quando o hub liga o modo autônomo de um projeto neste ambiente (`Project.autonomy_runner`, #181; veja "Modo autônomo e a branch `wip/autonoma`" em docs/hub.md), o ambiente cuida da branch de integração no clone do projeto, sem mexer na árvore de trabalho dele:

- **Ao aplicar a configuração:** cria a branch (padrão `wip/autonoma`) no `origin` a partir da base, se faltar, e abre o PR rascunho `branch → base` pela API do GitHub, com o token do ambiente. Sem integração com o GitHub, a branch sai e o relato avisa para abrir o PR à mão.
- **Sessão autônoma** (`session.start` com `autonomous: true`, ou "Sessão autônoma" na modal de início): antes de criar a worktree, o ambiente traz a base para a branch, num worktree temporário: `merge` (commit de merge, push sem força) ou `rebase` (push com `--force-with-lease`), conforme o projeto. Sem conflito, a worktree da issue sai da branch de integração, e o system prompt instrui o agente a abrir o PR (MR no GitLab) contra ela, com o comando da forja (`gh pr create --base wip/autonoma` ou `glab mr create -R <caminho> -b wip/autonoma`). A instrução vale também na retomada e nas sessões seguintes da mesma issue (evento `autonomy_session` no work item).
- **Prompt da sessão autônoma** (#191, ambiente 2.19): no `session.start` com `autonomous: true`, o `prompt` é opcional. Sem ele (ou vazio), o ambiente monta o prompt depois de decidir a branch e a worktree, com o template autônomo do projeto (`session_templates.autonomous`, vindo do hub) ou, vazio ou só com espaços, o padrão do `ct-core` (`DEFAULT_AUTONOMOUS_TEMPLATE`): trabalhar sem perguntar, registrar as decisões com origin `autonoma` e abrir o PR (ou MR) da branch da sessão para a de integração. Variáveis: `{{kind}}`, `{{number}}`, `{{ref}}` (`acme/acme-web#12` ou `ACME-123`), `{{owner}}`, `{{repo}}`, `{{title}}`, `{{branch}}`, `{{integration_branch}}`, `{{protected}}`, `{{change}}` (`PR` ou `MR`) e `{{closes}}` (a linha que liga a issue no corpo do PR: `Closes #12` no GitHub, a forma que o guarda-corpo exige no `gh pr create`, ou a chave `ACME-123` no Jira; o padrão pede essa linha); variável desconhecida fica como está, como na UI. Com `prompt`, vale o texto enviado. Sem `autonomous`, o `prompt` continua obrigatório. Um ambiente anterior à 2.19 recusa o `session.start` autônomo sem `prompt` (`-32602`).
- **Conflito:** nada é enviado ao `origin`, a sessão não começa (erro `-32050`, `AUTONOMY_CONFLICT`), o relato `conflict` sobe ao hub com os arquivos em conflito e sai o alerta `autonomy_conflict` (com um evento no work item do projeto). Resolva numa sessão do projeto, trazendo a base para a branch; o próximo alinhamento volta o relato para `ok`.
- **Alinhar sem iniciar:** `autonomy.align { project_id | owner+repo }` (também pelo canal do hub) faz só o alinhamento e devolve o relato.
- **Continuar depois do limite do plano** (#183, ambiente 2.20): `session.continue { session_id }` (também pelo canal do hub) escreve no PTY da sessão viva o texto fixo "O limite do plano renovou. Continue a issue de onde parou, sem perguntar." e Enter, e grava o evento `session_continued` no work item. É o que o agendador do piloto automático usa para retomar a sessão estacionada depois do reset; nunca leva texto livre. Sessão que não está viva neste daemon é recusada. Ambiente anterior à 2.20 responde `-32601` e o hub registra `resume_failed`.

Num projeto com o modo desligado, ou em outro ambiente, `session.start` com `autonomous: true` é recusado, e as sessões comuns seguem como sempre.

### Uso do plano (statusline do Claude Code)

O provisionamento registra `ctower statusline --v <n>` como `statusLine` no `settings.json` da pasta de configuração do Claude Code do ambiente (`CLAUDE_CONFIG_DIR` ou `~/.claude`). Para instalar ou atualizar à mão: `ctower statusline install`. Rodar de novo não muda nada; se já havia um `statusLine` de outra origem, ele passa a rodar dentro do nosso (fica em `~/.config/ctower/statusline-chain.json`) e a saída dele continua aparecendo.

A cada atualização de uma sessão interativa (do Control Tower ou aberta direto no terminal), o comando grava `~/.local/share/ctower/usage/<session_id>.json` (0600) só com os campos de uso: modelo, esforço, custo estimado pelo cliente, janela de contexto, cache de prompt e as janelas do plano (5h e semanal). Ficam de fora a pasta de trabalho, o transcript e os dados do repositório. Arquivos sem atualização há 30 dias são apagados. O statusline não roda em `claude -p`.

O `ctowerd` lê a conta do ambiente com `claude auth status` (relida a cada 5 minutos) e expõe `session.usage` (última leitura da sessão, ou `null`), `account.get` (conta, saúde do statusline e o plano pelas sessões do ambiente) e `env.statusline.install`. O `env.info` traz a saúde do statusline (instalado, versão, se encadeia outro). Ligado a um hub, o ambiente envia a conta e a leitura mais recente quando mudam; o hub agrupa os ambientes pela conta em `hub.plan.accounts` (docs/hub.md).

**Login por token** (`CLAUDE_CODE_OAUTH_TOKEN`, gerado por `claude setup-token`): o `claude auth status` responde `loggedIn: true` com `authMethod: "oauth_token"`, mas sem `orgId` nem `email`, e a conta ficaria desconhecida. Nesse caso, o `ctowerd` usa a conta declarada nas variáveis `CTOWER_CLAUDE_ORG_ID` e `CTOWER_CLAUDE_EMAIL` do processo dele (#179). Copie os dois valores do `claude auth status` de um ambiente com login normal na mesma conta (`claude auth status | jq -r '.orgId, .email'`): com os mesmos valores, a chave é a mesma e o hub junta as leituras. Sem login, a conta declarada não vale; com a conta na saída do `claude auth status`, a da saída vence. O ambiente autônomo em container passa as duas variáveis pelo compose ([ambiente-autonomo.md](ambiente-autonomo.md)).

Regra do reset: com `agora >= resets_at`, a janela já renovou; o percentual antigo não é mostrado nem usado em alerta até chegar leitura nova.

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
