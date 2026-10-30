# Hub como plano de controle, ambiente só execução (issue #173)

Spec do épico #183. Revisa a spec do ledger centralizado (`2026-10-16-ledger-centralizado.md`, épico #64) nas §2 (o que é compartilhado), §5 (replicação), §6 (hub fora do ar), §7 (protocolo e UI), §9 (migração) e §11 (configuração). Também revisa a §8 (segurança) e a §4 na regra de atributos de projeto. O que não aparece aqui continua valendo: identidade (`env_id`, `uid`, HLC), status do work item por HLC, ingestão idempotente, snapshots com `VACUUM INTO`.

As decisões confirmadas pelo usuário estão no épico #183. As que esta spec acrescentou foram revisadas e confirmadas pelo usuário em 2026-10-18 e estão na §12.

## 1. Problema e direção

No desenho do #64, o hub é um `ctowerd` com `hub = true`: roda sessões, envia o próprio ledger em processo e consolida os projetos que cada ambiente cadastrou por "o mais recente vence". Ninguém é dono da configuração do projeto.

Direção: separar o **plano de controle** do **plano de execução**.

```
                  cthubd (controle) — hub.db, snapshots
   projetos · configuração · atribuições · visão consolidada · GitHub de todos
     │ GET /sync/config?since=rev            ▲ POST /sync/push (outbox)
     │ (só o atribuído ao env_id do token)   │ POST /sync/query/decisions
     ▼                                       │
  ctowerd A (execução)                    ctowerd B (execução)
  cache da config · sessões · PTY · worktrees · hooks · MCP · ledger local podado

  UI ── /ws do hub (token de UI do hub): projetos, atribuições, árvore, histórico, dashboard
     └─ /ws de cada ambiente (token de UI do ambiente): sessões, terminal, recuperação
```

O local-first continua: o ambiente grava no `ledger.db` antes de agir e trabalha offline com a última configuração recebida.

## 2. Papéis

| | `cthubd` (controle) | `ctowerd` (execução) |
|---|---|---|
| Projetos e configuração | **origem**: cria, edita, exclui | cache somente leitura dos atribuídos |
| Atribuições projeto × ambiente | decide | aplica (clona ou valida) e relata |
| Sessões, PTY, worktrees, hooks, MCP | **não tem** | dono |
| Ledger | `hub.db` consolidado, única cópia do histórico | `ledger.db` local, só dos work items ativos (§6) |
| GitHub | sincroniza todos os projetos | sincroniza só os atribuídos (status local, guarda-corpo) |
| Backup | snapshots do `hub.db` (crítico) | — |

- `cthubd` é um crate binário novo (`crates/cthubd`). Reusa `ct-ledger::hub`, `ct-gh` e a parte comum de `ct-server` (HTTP/WS, autenticação por token). Não depende de `ct-sessions`, `ct-pty` nem `ct-git`.
- A máquina do hub que também executa projetos roda um `ctowerd` comum ao lado, ligado ao `cthubd` local como qualquer outro ambiente. Não existe mais envio "em processo".
- `ctowerd` deixa de abrir `hub.db`. Com `hub = true` no config, recusa subir com a mensagem `hub = true não existe mais; migre com "cthubd migrate" (docs/hub.md)`.

## 3. Modelo de dados do hub

Migrações novas do `hub.db` (a partir da 6), uma por sub-issue. As tabelas atuais continuam.

```
project            + rev INTEGER NOT NULL DEFAULT 0      -- config_rev da última mudança
                   + deleted_at INTEGER                  -- lápide (§3.2)
                   - attrs_hlc, attrs_env deixam de ser usados (ficam no esquema)

assignment         project_id, env_id, repo_path ('' = padrão do ambiente),
                   created_at, removed_at?, rev
                   PRIMARY KEY (project_id, env_id)

project_location   (existente) passa a ser só o relato do ambiente:
                   + state ('ok'|'error'), + error?, + applied_rev, + reported_at

push_quarantine    id, env_id, seq, kind, key, project_key, payload, received_at
                   UNIQUE (env_id, seq)
                   -- linhas de push de projeto desconhecido (§5)

hub_meta           + 'config_rev' (monotônico, nunca volta)
```

### 3.1 Projeto como origem

- `hub.projects.add { url, name?, base_branch?, protected_branches? }`: parse pelo mesmo código de `projects.parse_url`, sem clone. Valida com o token do GitHub do hub que o repositório existe e que `base_branch` existe. Chave `owner/repo` sem diferenciar maiúsculas, como hoje. Na mesma transação do cadastro, a quarentena do projeto (§5) é reaplicada pela ingestão normal, na ordem (`env_id`, `seq`), e as linhas reaplicadas saem da quarentena.
- `hub.projects.update { owner, repo, name?, base_branch?, protected_branches? }`.
- `hub.projects.delete { owner, repo, confirm? }` (§3.2).
- `hub.projects.list` continua, agora com as atribuições e o estado de cada uma.

### 3.2 Exclusão

Excluir no hub apaga o histórico do projeto no `hub.db` (a única cópia). Por isso:

1. Com atribuições ativas, o hub recusa sem `confirm: true` e lista os ambientes e as sessões vivas relatadas.
2. Antes de apagar, o hub tira um snapshot fora do ciclo (`hub-<data>-pre-delete.db`), que não entra na rotação.
3. Na transação: as atribuições recebem `removed_at` e `rev` novo, o histórico (work items, sessões, decisões, eventos, espelho do GitHub) é apagado com o projeto em `purging` (mesmo padrão da migração 3 do `ledger.db`), e a linha de `project` fica como **lápide** (`deleted_at`, `rev`). Com a lápide, as linhas atrasadas desse projeto que chegarem num push são descartadas, em vez de irem para a quarentena (§5). Ela também permite propagar a remoção.
4. Cadastrar de novo o mesmo `owner/repo` reaproveita a linha e limpa `deleted_at`.

### 3.3 Atribuição

- `hub.assignments.list { env?, owner?, repo? }`, `hub.assignments.add { owner, repo, env, repo_path? }`, `hub.assignments.remove { owner, repo, env, confirm? }`.
- `repo_path` vazio: o ambiente resolve como `<repos_root>/<repo>` (padrão `~/projetos/<repo>`, o mesmo do cadastro por URL da #40).
- Estado mostrado na UI, derivado de `assignment` × `project_location`:
  - **pendente:** `project_location.applied_rev` menor que `assignment.rev` (ou sem relato);
  - **aplicada:** relato `ok` com `applied_rev >= rev`;
  - **com erro:** relato `error`, com a mensagem.
- Remover com sessões vivas relatadas pelo ambiente exige `confirm: true`. O ambiente **nunca** encerra sessão por causa disso (§4.3).

### 3.4 `config_rev`

Toda mudança em projeto (inclusive lápide) ou atribuição faz, na mesma transação, `config_rev = config_rev + 1` e grava esse valor em `rev` da linha. É a base do pull: o ambiente pede "o que mudou desde `rev`".

## 4. Hub → ambiente: pull da configuração

### 4.1 Endpoint

`GET <hub_url>/sync/config?since=<rev>&epoch=<época>`, token de sync no `Authorization`. O hub resolve o `env_id` pelo token (o vínculo token × `env_id` de hoje). O ambiente manda também `X-Ctower-Env-Id`; divergência dá `403`, como no push.

Respostas:

- `since == config_rev` atual **e** `epoch == config_epoch` atual: `200 { "config_rev": N, "config_epoch": E, "changed": false }`.
- `config_epoch` (em `hub_meta`) é aleatória e sorteada de novo pelo `cthubd restore` (e criada pela migração). Um hub restaurado de snapshot pode voltar ao mesmo `config_rev` com outro conteúdo; com a época diferente (ou ausente no pedido), o `since` do ambiente não vale e a resposta é sempre o conjunto completo, com `removed` desde o início. O ambiente guarda `config_rev` e `config_epoch` do último pull e manda os dois.
- Caso contrário:

```json
{
  "config_rev": 42,
  "config_epoch": 81723645190,
  "changed": true,
  "projects": [
    { "owner": "deep-ion-ai", "repo": "control-tower", "name": "control-tower",
      "base_branch": "develop", "protected_branches": ["develop", "main", "master"],
      "repo_path": "", "rev": 40 }
  ],
  "removed": [ { "owner": "deep-ion-ai", "repo": "antigo", "rev": 41 } ]
}
```

- `projects` é **o conjunto completo** atribuído ao ambiente, não só o delta. Ele é pequeno (dezenas de linhas), e assim o cache se corrige sozinho mesmo depois de um hub restaurado de snapshot com `config_rev` menor que o do ambiente.
- `removed` traz as atribuições deste ambiente removidas com `rev > since` (e os projetos excluídos). O ambiente também trata como removido qualquer projeto do cache que não veio em `projects`.
- Projetos não atribuídos ao ambiente nunca aparecem, nem em `removed`.

### 4.2 Cache local

Migração nova do `ledger.db` (a partir da 6):

```
project   + config_rev INTEGER        -- rev do hub que gerou esta linha; NULL = legado (§9)
          + unassigned_at INTEGER     -- atribuição removida; a linha some da lista
          + hub_repo_path TEXT        -- repo_path pedido pelo hub ('' = <repos_root>/<repo>)
          + location_state, location_error, applied_rev   -- último relato, reenviado em toda fotografia
ledger_meta + 'config_rev', 'config_epoch', 'config_pulled_at'
```

- `project` vira **cache somente leitura**: só a tarefa de pull escreve nele. `projects.add`, `projects.update` e `projects.delete` saem do `/ws` do ambiente. `projects.list` lê o cache (sem `unassigned_at`).
- A tarefa de pull roda no boot, a cada `config_pull_interval_secs` (padrão 60) e sob demanda: `ctower hub sync` e o `hub.sync.now` puxam a configuração e depois enviam o outbox.
- Aplicar uma atribuição nova ou alterada: grava o cache, clona ou valida o clone em `repo_path` (reusando o fluxo da #40) e enfileira no outbox um relato `project_location { owner, repo, repo_path, state, error?, applied_rev }`. O pull só grava o cache; a aplicação (clone) roda à parte, uma por vez, e nem o pull seguinte nem o envio do outbox esperam por ela. O caminho efetivo é resolvido de novo em todo pull: se mudou (ex.: `repos_root` novo no ambiente), a atribuição volta a pendente localmente e é reaplicada e relatada. Mudar `base_branch` ou as protegidas só atualiza o cache, sem reiniciar o daemon. O guarda-corpo lê o cache.
- Remover: grava `unassigned_at`. O clone e os worktrees ficam no disco. A UI do ambiente mostra o que restou para limpar à mão. Os work items do projeto entram na poda (§6).
- O espelho do GitHub do ambiente (`issue_cache`, `pr_cache`) só sincroniza projetos atribuídos.

### 4.3 Offline

Hub fora do ar: o ambiente usa a última configuração em cache, sem limite de idade. Sessões, hooks e guarda-corpo seguem normalmente. O pull tenta de novo com o mesmo backoff do envio (até 5 min).

`env.info` ganha `hub_config { config_rev, pulled_at, last_error }`. A UI mostra a idade da configuração ("configuração do hub de há 3 h").

## 5. Ambiente → hub: execução pelo outbox

O outbox e o `POST /sync/push` de hoje continuam (§5 da spec antiga), com estas mudanças:

- Não há mais `HubTarget::Local`: todo envio vai por HTTP a `hub_url`.
- **Atributos de projeto vindos do ambiente são ignorados** (sai a regra "vale o mais recente" da §4 da spec antiga). Do ambiente, o hub aceita a execução (work items, sessões, decisões, eventos, revisões) e o relato `project_location`.
- **O hub nunca cria projeto a partir de um push.** Cada linha do lote é tratada pelo projeto (`owner/repo`) a que pertence:
  - **projeto cadastrado:** ingestão normal, sem os atributos de projeto;
  - **projeto com lápide:** a linha é descartada (contada em `IngestResult`, com aviso no log);
  - **projeto desconhecido, sem lápide:** a linha vai para `push_quarantine` (o payload intacto, com `env_id` e `seq`), com aviso no log e em `hub.status` (contagem por projeto e por ambiente).
- Nos três casos, a linha **recebe ack**: o `last_seq` devolvido cobre o lote inteiro. Assim, o outbox do ambiente não trava e a poda não fica parada. As linhas em quarentena estão no `hub.db` e entram nos snapshots.
- Quando o projeto é cadastrado no hub (`hub.projects.add`), a quarentena dele é reaplicada (§3.1). `hub.quarantine.list` mostra o que está em quarentena; `hub.quarantine.discard { owner, repo }` descarta, com confirmação.
- `project_removed` vindo do ambiente deixa de existir: só o hub remove.
- O MCP `ledger_list_decisions` com escopo `epic`/`project` continua consultando o hub (`POST /sync/query/decisions`) e devolve as locais com `partial: true` se ele não responder. Com a poda, a resposta local fica mais parcial; o aviso já cobre isso.

## 6. Poda após o ack

O ambiente guarda só o necessário para executar. O hub vira a única cópia do histórico.

### 6.1 Critério

Um work item é podado quando **tudo** vale:

1. **Não está ativo:** status `concluido` **ou** o projeto está com `unassigned_at`; e não tem sessão em `starting`, `running`, `idle` ou `interrupted` (recuperação pendente é ativa).
2. **Confirmado pelo hub:** a poda só roda com o `sync_outbox` vazio, verificado dentro da transação dela. O outbox só esvazia com o ack do hub, então tudo do work item já está lá. A tarefa de poda roda logo depois de um envio que zerou o outbox.
3. **Velho:** `closed_at` (ou `unassigned_at`) e o `last_event_at` da sessão mais recente têm mais de `retention_days` (padrão **30**; `0` desliga a poda).
4. **Fora da revisão local:** a revisão das decisões (`review.get`, `review.mark_reviewed`) só existe no ambiente. Não é podado o work item que tem decisão `autonoma` sem revisão (nem o épico com decisão pendente numa sub-issue dele), nem a sub-issue cujo épico pai ainda tem work item não concluído no ambiente (a revisão do épico junta as decisões das sub-issues). (Revisão do PR #192.)

Work items `bloqueado`, `pausado`, `backlog` etc. contam como ativos e nunca são podados enquanto o projeto estiver atribuído.

### 6.2 O que sai

Numa transação: `event`, `decision`, `pr_cache`, `session` e `work_item` desse work item. Depois do commit, os arquivos locais das sessões dele (`<data>/sessions/<id>` e os backups em `<data>/transcripts`). Worktrees e branches não são tocados (são do git). Projeto com `unassigned_at` e sem work items restantes: a linha do cache e o espelho do GitHub dele também saem.

Se o work item voltar a ter sessão depois (issue reaberta), o ambiente cria uma linha nova pela chave natural, e o hub funde como hoje. Os `uid` determinísticos dos eventos do GitHub (`gh:...`) apagados deixam o maior sufixo usado em `pruned_uid (base, max_n)`; uma transição repetida depois da poda (issue reaberta e fechada de novo) recebe o sufixo seguinte (`:2`, `:3`...), em vez de um `uid` que o hub já tem e descartaria (`ON CONFLICT (uid) DO NOTHING`).

### 6.3 Exceção nas triggers

Mesmo padrão da `purging` (migração 3). Uma migração nova do `ledger.db` cria `pruning (work_item_id INTEGER PRIMARY KEY)` e recria `event_no_delete` e `decision_no_delete` aceitando o DELETE também quando o `work_item_id` da linha está em `pruning`. A transação insere o id em `pruning`, apaga e remove o id antes do commit. Fora dela, as triggers continuam bloqueando.

### 6.4 `ctower hub resync`

Continua colocando no outbox tudo o que o ledger local tem, mas agora isso é **só o que não foi podado** (work items ativos e os recentes). Há dois caminhos para reconstruir um hub perdido: restaurar um snapshot, ou cadastrar de novo os projetos no hub e rodar o resync. O resync também completa o que veio depois do snapshot. O que chegar antes do cadastro do projeto fica em quarentena (§5) e é reaplicado quando o projeto for cadastrado. O comando avisa quantos work items já foram podados.

### 6.5 Backup crítico

- `cthubd` compara o dispositivo (`st_dev`) de `backup_dir` com o do `hub.db`. Se for o mesmo, avisa no log no boot e em `hub.status` (a UI mostra a faixa "backup no mesmo disco do hub").
- Snapshot falho ou mais velho que 2 × `backup_interval_hours` também aparece como aviso em `hub.status`.

## 7. Hub obrigatório

- `ctowerd` sem `hub_url` ou sem `hub.token`: sobe, mas sem projetos. `env.info` traz `hub_config.linked = false`, e a UI mostra "ambiente não ligado a um hub". `session.start` e `session.start_project` são recusados com `ambiente não ligado a um hub: configure hub_url e grave o token com "ctower hub set-token"`. Hooks, MCP e `session.resume` de sessões já existentes continuam (para não perder trabalho no meio da migração).
- Projeto fora do cache (não atribuído): `session.start` recusa com `projeto não atribuído a este ambiente`.
- Sessão de planejamento no projeto (#43) e sessões de issue continuam iguais, só para projetos atribuídos.
- **Máquina única:** `provision.sh --hub --env` instala os dois serviços. O `cthubd` escuta em `127.0.0.1:7879`, o `ctowerd` em `127.0.0.1:7878`. A instalação gera o token de sync (`cthubd add-env <nome>`), grava `hub.token` no ambiente e `hub_url = "http://127.0.0.1:7879"` no config.

## 8. Protocolo e UI

- O `env.info` perde `hub: bool` e ganha `hub_config` (§4.3). Como saem métodos e campos, o protocolo do ambiente passa a **2.0**. O `/ws` do hub tem protocolo próprio, que começa em `1.0` e aparece em `hub.info { protocol, version, commit, name, config_rev, backup }`.
- Métodos do `/ws` do hub: os `hub.*` de hoje (`hub.projects.list`, `hub.tree.get`, `hub.workitem.history`, `hub.workitem.set_status`, `hub.dashboard.get`, `hub.envs.list`), mais `hub.info`, `hub.status`, `hub.projects.add|update|delete`, `hub.assignments.list|add|remove`, `hub.quarantine.list|discard`, `hub.envs.add|revoke` e `github.status|set_token|clear_token` (o token do GitHub do hub).
- Métodos que saem do `/ws` do ambiente: `projects.add`, `projects.update`, `projects.delete` e todos os `hub.*`, exceto `hub.sync.now` (que puxa a configuração e envia o outbox).
- A UI ganha uma **conexão com o hub** (URL e token de UI do hub) separada das conexões com os ambientes. Para iniciar uma sessão, ela escolhe um ambiente onde o projeto está atribuído e chama `session.start` nele, com (`owner/repo`, número).
- Hub fora do ar na UI: aviso "hub indisponível" e as telas dos ambientes seguem com o cache.

## 9. Segurança

Três escopos de token mais o de hook, cada um num arquivo `0600`:

| Token | Onde é gerado | Dá acesso a |
|---|---|---|
| UI do hub | `~/.config/cthub/ui.token`, na instalação do `cthubd` | `/ws` do hub |
| Sync (um por ambiente) | `cthubd add-env <nome>`; o hub guarda só o hash, o ambiente grava em `~/.config/ctower/hub.token` | `/sync/push`, `/sync/config`, `/sync/query/decisions`, preso ao `env_id` |
| UI do ambiente | `~/.config/ctower/ui.token` (como hoje) | `/ws` e `/hub/sync` do ambiente |
| Hook/MCP | `~/.config/ctower/hook.token` (como hoje) | `/hooks` e `/mcp` do ambiente |

- **Token preso na geração (#195):** `cthubd add-env <nome>` já prende o token a um `env_id`: um nome existente mantém o seu, um nome novo ganha um gerado no hub, e os dois voltam juntos para o ambiente gravar. O hub recusa (`403`) um `X-Ctower-Env-Id` que não bata com o preso, inclusive no primeiro uso. O bind no primeiro uso resta só para tokens de instalações antigas (sem `env_id`), com aviso no log.
- O token de sync só lê a configuração dos projetos atribuídos ao seu `env_id`. Revogar (`cthubd revoke-env`) corta push e pull.
- O token do GitHub do hub fica em `~/.config/cthub/github.token` (ou `gh auth token`). Cada ambiente mantém o próprio, para clonar e para o espelho local.
- Transporte: Tailscale, sem TLS próprio, como hoje. Bind fora de `127.0.0.1` só explícito.
- **Secrets** (#171) ficam no épico deles, que passa a ter o hub como origem. Esta spec não define o encaixe.

## 10. Configuração e diretórios do `cthubd`

- Config: `~/.config/cthub/config.toml` (`CTHUB_CONFIG_DIR`). Tokens e `github.token` no mesmo diretório.
- Dados: `~/.local/share/cthub/` (`CTHUB_DATA_DIR`): `hub.db` e, por padrão, `backups/`.

```toml
# ~/.config/cthub/config.toml
name = "mac-mini"
port = 7879                        # padrão; CTHUB_PORT sobrepõe
bind = "100.x.y.z"                 # IP Tailscale (explícito); 127.0.0.1 fica sempre no ar
gh_sync_interval_secs = 180
backup_dir = "/Volumes/Backup/cthub"   # vazio = ~/.local/share/cthub/backups
backup_interval_hours = 6          # 0 desliga (com aviso)
backup_keep = 28
```

`ctowerd` (`~/.config/ctower/config.toml`):

```toml
env_name = "wsl-pc"
hub_url = "http://mac-mini.tailnet:7879"   # obrigatório para executar projetos
config_pull_interval_secs = 60
repos_root = "~/projetos"          # repo_path vazio na atribuição = <repos_root>/<repo>
retention_days = 30                # poda; 0 desliga
# saem: hub, hub_backup_dir, hub_backup_interval_hours, hub_backup_keep
```

Subcomandos (no próprio binário `cthubd`, que funciona com o serviço no ar, por SQLite com WAL): `cthubd serve` (padrão do serviço), `cthubd add-env <nome>`, `cthubd revoke-env <nome>`, `cthubd migrate --from <data_dir>`, `cthubd restore <snapshot>`. Saem do `ctower`: `hub add-env` e `hub revoke-env`. Ficam: `hub set-token`, `hub sync`, `hub resync`.

## 11. Migração da instalação atual (#182)

Hoje: um `ctowerd` com `hub = true`, o `hub.db` com os tokens de sync, e ambientes ligados a ele, com projetos criados em cada um.

1. **Atualizar os `ctowerd` dos outros ambientes** para a versão nova antes de trocar o hub. O ambiente novo ainda envia ao hub antigo (o push não muda) e só passa a puxar configuração quando `/sync/config` responde; `404` é tratado como "hub antigo", e os projetos locais legados continuam valendo até o primeiro pull.
2. **Na máquina do hub**, nesta ordem (o `ctowerd` novo recusa subir com `hub = true`, e o atualizador faria rollback): instalar o `cthubd`, parar o `ctowerd` e rodar `cthubd migrate --from ~/.local/share/ctower`. O comando:
   - copia o `hub.db` para `~/.local/share/cthub/hub.db` (o original fica intocado como backup) e aplica as migrações novas;
   - mantém `env`, `env_token` (hashes e `env_id`), então os tokens de sync dos ambientes continuam valendo;
   - promove os projetos a origem e cria uma atribuição por `project_location` não removida, com o `repo_path` relatado;
   - inicializa `config_rev` e o `rev` de cada linha;
   - gera o token de UI do hub e cria o `config.toml` do `cthubd` a partir de `bind` e `hub_backup_*` do `ctowerd`;
   - gera um token de sync para o próprio `ctowerd` da máquina, preso ao `env_id` que ele já usava (o hub antigo gravava o ledger dele em processo com esse `env_id`, sem token), grava o `hub.token` dele e troca no config `hub = true` por `hub_url = "http://127.0.0.1:7879"` (o arquivo antigo vai para `config.toml.pre-cthub`);
   - recusa com o `ctowerd` ou o `cthubd` no ar e, sem `--force`, se o `cthubd` já tem `hub.db`. Monta o `hub.db` novo ao lado e só troca no fim; rodar de novo retoma uma migração interrompida (só se o `hub.db` antigo não mudou desde a cópia; senão exige `--force`) ou não faz nada.
3. Subir o `cthubd`, atualizar o `ctowerd` da máquina do hub e subi-lo. Nos outros ambientes, trocar a porta de `hub_url` para `7879` e reiniciar.
4. **Primeiro pull em cada ambiente:** linhas legadas (`config_rev` nulo) que vieram no pull são casadas por `owner/repo` e mantêm o `id` local (o histórico local continua ligado a elas). Legadas que o hub não atribuiu a este ambiente ganham `unassigned_at` e aparecem na UI como "projeto local fora do hub", sem nada ser apagado do disco.
5. Conferir na UI (ou `hub.envs.list`) os ambientes com 0 pendências e as atribuições "aplicadas".

O roteiro passo a passo fica em `docs/hub.md`.

## 12. Decisões

Escolhas desta spec que não estavam decididas no épico. O usuário as revisou em 2026-10-18 e confirmou todas, exceto a 3, que ele substituiu.

1. **Pull devolve o conjunto completo, não só o delta** (§4.1). Motivo: o conjunto é pequeno e o cache se corrige sozinho depois de um hub restaurado de snapshot; `removed` continua vindo para a UI e os logs.
2. **Exclusão de projeto no hub deixa lápide e tira snapshot antes** (§3.2). Motivo: o hub é a única cópia; a lápide impede que um push atrasado recrie o projeto.
3. ~~Projeto desconhecido no push é criado a partir do ambiente.~~ **Substituída pela decisão do usuário (ledger, decisões 119 e 120):** o hub nunca cria projeto a partir de push. Linhas de projeto desconhecido e sem lápide recebem ack e vão para a `push_quarantine`, com aviso; são reaplicadas quando o projeto for cadastrado no hub (§3.1, §5). Hub perdido: restaurar um snapshot, ou cadastrar os projetos à mão e rodar `ctower hub resync`.
4. **Poda só com o outbox vazio** (§6.1), em vez de rastrear o `seq` de cada linha do work item. Motivo: correto por construção e simples; o outbox esvazia em segundos com o hub no ar.
5. **`retention_days = 30`, contado de `closed_at`/`unassigned_at` e da última atividade de sessão; `interrupted` conta como ativo** (§6.1). Motivo: um mês cobre a revisão de PR e a recuperação; nunca podar trabalho que espera retomada.
6. **Pull a cada 60 s** (`config_pull_interval_secs`), com o mesmo backoff do envio. Motivo: mudança de config chega rápido e o custo é uma requisição pequena com `changed: false`.
7. **Sem hub, `session.resume` e hooks seguem; só `session.start` é recusado** (§7). Motivo: não perder trabalho em andamento durante a migração ou um erro de config.
8. **Subcomandos no binário `cthubd`, sem um CLI `cthub` separado** (§10). Motivo: um artefato a menos no release e no atualizador.
9. **Porta 7879, `repos_root = "~/projetos"` configurável no ambiente.** Motivo: porta distinta permite hub e ambiente na mesma máquina; o padrão de caminho é o da #40.
10. **Protocolo do ambiente 2.0; hub com protocolo próprio a partir de 1.0** (§8). Motivo: saem métodos e o campo `hub`, o que quebra clientes antigos.
11. **Ambientes atualizados antes do hub na migração, com `404` em `/sync/config` = hub antigo** (§11). Motivo: a troca do hub não exige parar todos os ambientes ao mesmo tempo.
12. **O token do GitHub do hub é separado dos ambientes** (§9). Motivo: o hub valida e sincroniza todos os projetos; cada ambiente continua clonando com o seu.

## 13. Próximos passos

Sub-issues do épico #183: #174 (binário `cthubd`), #175 (projetos no hub), #176 (atribuições), #177 (pull da configuração), #178 (ambiente sem cadastro, hub obrigatório), #179 (poda), #180 (UI), #181 (instalação e release), #182 (migração).
