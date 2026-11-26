# Providers: como adicionar um tipo de conexão

Um **provider** é um tipo de serviço externo (hoje GitHub, GitLab e Jira). Uma **conexão** é um cadastro desse tipo no hub (URL ou host, fonte da credencial e opções próprias), e os projetos a referenciam. Este guia mostra onde cada parte mora e o que falta para um tipo novo. O modelo de cadastro e a operação estão em [`hub.md`](hub.md), seção 2, "Conexões".

Estado: o descritor por tipo, a conexão no hub, a migração dos cadastros antigos, a materialização no ambiente, o resolvedor de credencial (comando, secret e arquivo para Jira e GitLab, #274; o token do GitHub no hub e no ambiente, #275) a tela **Hub › Conexões** e a sobrescrita por conexão editável no ambiente (#276) existem.

## Onde fica cada parte

| Parte | Arquivo | O que tem |
| --- | --- | --- |
| Tipos e descritores | `crates/ct-core/src/provider.rs` | `ProviderKind`, `Connection`, `CredentialSource`, `Capabilities`, `Readiness`, o trait `ProviderDescriptor`, os descritores `GITHUB`, `GITLAB`, `JIRA`, o registro `PROVIDERS`, `descriptor(kind)`, `Role` e `check_role` |
| Cadastro da conexão no hub | `crates/ct-hub-store/src/connection.rs` | `add`, `update`, `delete` (recusa conexão em uso), `test` (sem rede), `check_project_connections`, `env_connections` (o que vai no `GET /sync/config`) |
| RPC | `crates/cthubd/src/ws.rs` (`connections_call`) e `crates/cthubd/src/methods.rs` | `hub.connections.list|add|update|delete|test` |
| Referência no projeto | `crates/ct-core/src/tracker.rs` | `JiraConfig.connection`, `GitlabConfig.connection` e `GithubConfig.connection` (`Option<i64>`; o GitHub usa a mesma no rastreador e na forja, `github_connection`), `jira_cli_prefix` e `glab_cli_prefix` |
| Migração dos cadastros antigos | `crates/ct-hub-store/src/legacy_connections.rs` | cria as conexões a partir de `url`/`host` e `credential_cmd`, idempotente, com cópia do `hub.db` antes |
| Materialização no ambiente | `crates/ct-ledger/src/hub_config.rs` (`materialize_connections`) e `crates/ct-server/src/hub_config.rs` (`apply_connections`) | no pull, preenche `url`, `host`, `credential_cmd` e `config_file` do projeto a partir da conexão |
| `[connections]` do `config.toml` | `crates/ct-server/src/env_config.rs` e `crates/ct-core/src/credentials.rs` | a sobrescrita do comando por conexão (`ConnectionOverrides::source`); `env.connections.get|set` (`connection_rows`, `EnvConfig::set_connection`); `convert_credentials_file` converte `[credentials]`; `migrate_github_token` leva o `github.token` do ambiente para a sobrescrita da conexão GitHub |
| Fonte efetiva no ambiente | `crates/ct-core/src/credentials.rs` (`SharedCredentials::jira_source`, `gitlab_source`) | a fonte de cada projeto: `[credentials]` do projeto, a conexão do hub com a sobrescrita, ou o comando do cadastro; o pull e o boot atualizam as conexões em uso (`set_connections`) |
| Resolvedor de credencial | `crates/ct-proc/src/resolver.rs` | `CredentialResolver` (cache em memória), `Ask`, `resolve()` com um braço por `CredentialSource`. O ambiente (`crates/ct-sessions/src/resolver.rs`, com as chaves do cache) e o hub (`crates/cthubd/src/github.rs`) usam o mesmo |
| Token do GitHub | `crates/ct-gh/src/lib.rs` (`TokenProvider`, `GhClient::with_provider`), `crates/ct-sessions/src/github.rs` (`GithubSync::client_for`) e `crates/cthubd/src/github.rs` (`HubGithub::client_for`) | o cliente do GitHub de cada projeto pede o token ao resolvedor, com a fonte da conexão `github` (#275) |
| UI | `ui/src/hub/connections.ts`, `ui/src/hub/ConnectionsPanel.tsx`, `ui/src/project/settings/TrackerForm.tsx`, `GeneralForm.tsx` e `ui/src/settings/ConnectionOverridesSettings.tsx` | formulário e rótulos por tipo, tela Conexões, escolha da conexão no projeto, sobrescrita no ambiente (Configurações › Ambiente › Credenciais das conexões) |

## Os cinco elementos de uma conexão

- **`ProviderKind`**: o tipo (`github`, `gitlab`, `jira`). Vai no fio em minúsculas.
- **`endpoint`**: URL (Jira), host (GitLab) ou API (GitHub, vazio = o padrão do tipo).
- **`CredentialSource`**: de onde vem o token. As variantes são `secret` (nome de um secret do hub), `command` (a primeira linha da saída é o token), `cli` (`gh auth token`), `file` (caminho) e `none`. Ela é serializada com `kind` na tag, por exemplo `{ "kind": "command", "cmd": "pass show jira/token | head -1" }`.
- **`options`**: específico do tipo, validado pelo descritor (o Jira usa `config_file`).
- **`Capabilities`**: o que o tipo serve, `tracker` (issues) e/ou `forge` (repositório, PRs e MRs). É isso que `check_role` confere quando um projeto referencia a conexão.

## Passo a passo para um tipo novo

Use o Jira como modelo: `JiraDescriptor` em `provider.rs`.

1. **Tipo.** Acrescente a variante em `ProviderKind`, pela macro `str_enum!`, com o nome do fio. A tabela `connection` guarda `kind` como `TEXT` sem `CHECK`, então o hub não precisa de migração para o tipo novo. A validação é do descritor.

2. **Descritor.** Implemente `ProviderDescriptor` para o tipo:
   - `kind`, `label` e `capabilities`;
   - `default_endpoint`, se houver um padrão;
   - `token_env`, o nome da variável que leva o token (`GH_TOKEN`, `GITLAB_TOKEN`, `JIRA_API_TOKEN`);
   - `validate`, que confere tipo, opções e fonte. Os auxiliares `check_kind`, `check_options` e `check_token_source` (Jira e GitLab) já existem e servem de base. É aqui que se decide quais formas de credencial o tipo aceita;
   - `session_prefix`, o prefixo das instruções da sessão, como `JIRA_API_TOKEN="$(<cmd>)" jira`. O comando vem de `source_cmd`: o próprio comando, `ctower secret get NOME` para o `secret` e a leitura da primeira linha não vazia do arquivo (`file_cmd`, com `~` como `"$HOME"`) para o `file`, o mesmo token que o resolvedor lê.
   - `readiness` tem implementação padrão pelo `validate`. Não faz rede.

3. **Registro.** Inclua o descritor como `static` e na lista `PROVIDERS`. `descriptor(kind)` falha se o tipo não estiver ali, e o teste `registro_tem_um_descritor_por_tipo` pega isso.

4. **Fonte de credencial.** As cinco variantes de `CredentialSource` já existem. Uma variante nova exige:
   - um braço em `resolve()` (`crates/ct-proc/src/resolver.rs`) e em `source_cmd` (`provider.rs`), que devem dar o mesmo token;
   - o formulário e os rótulos da UI (`CRED_KINDS` e `CRED_TEXT`);
   - a regeneração dos tipos TypeScript (passo 7).

5. **Resolução no ambiente.** Quem usa o token chama `CredentialResolver::token` ou `with_refresh`, com um `Ask`: a chave do cache (`jira:<projeto>`, `gitlab:<projeto>`, `github:<projeto>`), o nome para as mensagens, a fonte, a busca de secret, o prazo do comando e o comando do `cli` (o GitHub passa o `gh` configurado, `gh_bin`). A fonte vem de `Sessions::jira_source`/`gitlab_source` (a conexão do hub com a sobrescrita do ambiente), e a busca de secret é `Sessions::secret_lookup`, que lê o cache de secrets do ambiente no projeto (a entrada do projeto sobrepõe a global). Um tipo novo precisa de um ponto de chamada próprio. O mascaramento das sessões (`connection_mask`, em `crates/ct-sessions/src/secrets.rs`) só lê o cache com `CredentialResolver::cached` e nunca resolve: abrir uma sessão não roda comando de credencial. Cada token resolvido de novo atualiza o mascaramento das sessões vivas (`Sessions::refresh_masks`). Um tipo novo cujo token deva ser mascarado entra em `connection_mask`.

6. **Projeto e materialização.** Para um tipo que o projeto referencia, o cadastro do projeto ganha o campo `connection` (como `JiraConfig.connection`). O hub confere a referência em `check_project_connections`, e `materialize_connections` precisa de um ramo que preencha os campos do projeto a partir da conexão. Sem esse ramo, o ambiente não usa a conexão.

7. **Tipos TypeScript.** Regenere os tipos que a UI importa:
   ```
   cargo test -p ct-server --features ts export_ts
   cargo test -p cthubd --features ts export_ts
   ```
   Depois versione `ui/src/protocol/gen` e `ui/src/protocol/hub-gen`. O CI (job "Tipos TypeScript da UI em dia", em `.github/workflows/ci.yml`) falha se o gerado não bater com o Rust.

8. **UI.** Em `ui/src/hub/connections.ts`, acrescente o tipo a `CONNECTION_KINDS`, e preencha `KIND_LABEL`, `CRED_KINDS`, `ENDPOINT_HINT` e `CMD_HINT` (com exemplos fictícios). Em `ConnectionsPanel.tsx`, os rótulos das formas de credencial vêm de `CRED_TEXT`. Na escolha do projeto, `TrackerForm.tsx` lista as conexões pelo tipo com `connectionsOfKind`.

## O que ainda não existe

- **Token de Jira e GitLab no hub.** O hub só resolve o token do GitHub (#275). Ele não fala com o Jira nem com o GitLab e só monta prefixos de sessão (o `issue_view` do gestor, as telas), com o comando de `source_cmd`. O `secret` desses prefixos resolve na sessão, no ambiente (`ctower secret get`).
- **Prefixo pelo descritor.** Os templates não chamam o `session_prefix` da conexão. Ele só chega ao prompt pela ponte `Connection::from_jira` e `Connection::from_gitlab`, usada por `jira_cli_prefix` e `glab_cli_prefix`, que leem o `credential_cmd` que a materialização preencheu com o `source_cmd` da conexão.
- **Comportamento por tipo.** Sync, leitura de issue e branches ainda usam `match` por tipo. O descritor cobre configuração e credencial, não o comportamento.
- **Campos antigos.** `url`, `host`, `credential_cmd` e `config_file` seguem nos structs `JiraConfig` e `GitlabConfig`, como ponte.

Nenhum exemplo deste guia usa dado real: os nomes são fictícios (`jira-empresa`, `jira.example.com`).
