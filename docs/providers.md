# Providers: como adicionar um tipo de conexão

Um **provider** é um tipo de serviço externo (hoje GitHub, GitLab e Jira). Uma **conexão** é um cadastro desse tipo no hub (URL ou host, fonte da credencial e opções próprias), e os projetos a referenciam. Este guia mostra onde cada parte mora e o que falta para um tipo novo. O modelo de cadastro e a operação estão em [`hub.md`](hub.md), seção 2, "Conexões".

Estado: o descritor por tipo, a conexão no hub, a migração dos cadastros antigos, a materialização no ambiente, o resolvedor de credencial do ambiente e a tela **Hub › Conexões** existem. O resolvedor do hub e a sobrescrita editável por conexão não existem ainda (veja "O que ainda não existe").

## Onde fica cada parte

| Parte | Arquivo | O que tem |
| --- | --- | --- |
| Tipos e descritores | `crates/ct-core/src/provider.rs` | `ProviderKind`, `Connection`, `CredentialSource`, `Capabilities`, `Readiness`, o trait `ProviderDescriptor`, os descritores `GITHUB`, `GITLAB`, `JIRA`, o registro `PROVIDERS`, `descriptor(kind)`, `Role` e `check_role` |
| Cadastro da conexão no hub | `crates/ct-hub-store/src/connection.rs` | `add`, `update`, `delete` (recusa conexão em uso), `test` (sem rede), `check_project_connections`, `env_connections` (o que vai no `GET /sync/config`) |
| RPC | `crates/cthubd/src/ws.rs` (`connections_call`) e `crates/cthubd/src/methods.rs` | `hub.connections.list|add|update|delete|test` |
| Referência no projeto | `crates/ct-core/src/tracker.rs` | `JiraConfig.connection` e `GitlabConfig.connection` (`Option<i64>`), `jira_cli_prefix` e `glab_cli_prefix` |
| Migração dos cadastros antigos | `crates/ct-hub-store/src/legacy_connections.rs` | cria as conexões a partir de `url`/`host` e `credential_cmd`, idempotente, com cópia do `hub.db` antes |
| Materialização no ambiente | `crates/ct-ledger/src/hub_config.rs` (`materialize_connections`) e `crates/ct-server/src/hub_config.rs` (`apply_connections`) | no pull, preenche `url`, `host`, `credential_cmd` e `config_file` do projeto a partir da conexão |
| `[connections]` do `config.toml` | `crates/ct-server/src/env_config.rs` e `crates/ct-core/src/credentials.rs` | a sobrescrita do comando por conexão; `convert_credentials_file` converte `[credentials]` |
| Resolvedor de credencial | `crates/ct-sessions/src/resolver.rs` | `CredentialResolver` (cache em memória), `Ask`, `resolve()` com um braço por `CredentialSource` |
| UI | `ui/src/hub/connections.ts`, `ui/src/hub/ConnectionsPanel.tsx`, `ui/src/project/settings/TrackerForm.tsx` e `GeneralForm.tsx` | formulário e rótulos por tipo, tela Conexões, escolha da conexão no projeto |

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
   - `session_prefix`, o prefixo das instruções da sessão, como `JIRA_API_TOKEN="$(<cmd>)" jira`. Quando a fonte é `secret`, o prefixo usaria `$(ctower secret get NOME)`.
   - `readiness` tem implementação padrão pelo `validate`. Não faz rede.

3. **Registro.** Inclua o descritor como `static` e na lista `PROVIDERS`. `descriptor(kind)` falha se o tipo não estiver ali, e o teste `registro_tem_um_descritor_por_tipo` pega isso.

4. **Fonte de credencial.** As cinco variantes de `CredentialSource` já existem. Uma variante nova exige:
   - um braço em `resolve()` (`crates/ct-sessions/src/resolver.rs`);
   - o formulário e os rótulos da UI (`CRED_KINDS` e `CRED_TEXT`);
   - a regeneração dos tipos TypeScript (passo 7).

5. **Resolução no ambiente.** Quem usa o token chama `CredentialResolver::token` ou `with_refresh`, com um `Ask`: a chave do cache (`jira:<projeto>`, `gitlab:<projeto>`), o nome para as mensagens, a fonte, a busca de secret e o prazo do comando. Hoje Jira e GitLab passam `no_lookup` como busca de secret, então `secret` não resolve neles no ambiente (veja abaixo). Um tipo novo precisa de um ponto de chamada próprio.

6. **Projeto e materialização.** Para um tipo que o projeto referencia, o cadastro do projeto ganha o campo `connection` (como `JiraConfig.connection`). O hub confere a referência em `check_project_connections`, e `materialize_connections` precisa de um ramo que preencha os campos do projeto a partir da conexão. Sem esse ramo, o ambiente não usa a conexão.

7. **Tipos TypeScript.** Regenere os tipos que a UI importa:
   ```
   cargo test -p ct-server --features ts export_ts
   cargo test -p cthubd --features ts export_ts
   ```
   Depois versione `ui/src/protocol/gen` e `ui/src/protocol/hub-gen`. O CI (job "Tipos TypeScript da UI em dia", em `.github/workflows/ci.yml`) falha se o gerado não bater com o Rust.

8. **UI.** Em `ui/src/hub/connections.ts`, acrescente o tipo a `CONNECTION_KINDS`, e preencha `KIND_LABEL`, `CRED_KINDS`, `ENDPOINT_HINT` e `CMD_HINT` (com exemplos fictícios). Em `ConnectionsPanel.tsx`, os rótulos das formas de credencial vêm de `CRED_TEXT`. Na escolha do projeto, `TrackerForm.tsx` lista as conexões pelo tipo com `connectionsOfKind`.

## O que ainda não existe

- **Resolvedor de credencial no hub.** Nenhum código do hub chama o `CredentialResolver`. Uma conexão com `secret` é gravada, mas o hub não lê o valor.
- **Secret no ambiente para Jira e GitLab.** A busca de secret é `no_lookup` nos dois. O ambiente recebe o `command` da conexão (ou a sobrescrita); sem comando, ele avisa ("não tem comando de credencial neste ambiente") e o projeto fica sem credencial.
- **Prefixo pelo descritor.** Os templates não chamam o `session_prefix` da conexão. Ele só chega ao prompt pela ponte `Connection::from_jira` e `Connection::from_gitlab`, usada por `jira_cli_prefix` e `glab_cli_prefix`, que leem o cadastro do projeto.
- **Sobrescrita por conexão editável.** O ambiente lê `[connections."nome"] credential_cmd`, mas não há tela nem RPC para editá-la. A tela de credenciais do ambiente ainda grava `[credentials."owner/repo"]`, que o pull converte.
- **Comportamento por tipo.** Sync, leitura de issue e branches ainda usam `match` por tipo. O descritor cobre configuração e credencial, não o comportamento.
- **GitHub.** O `GithubDescriptor` existe, mas o `ct-gh` segue com o próprio caminho de token (`github.token`, `gh auth token`, `github.set_token`). A tela `GithubSettings` também continua.
- **Campos antigos.** `url`, `host`, `credential_cmd` e `config_file` seguem nos structs `JiraConfig` e `GitlabConfig`, como ponte.

Nenhum exemplo deste guia usa dado real: os nomes são fictícios (`jira-empresa`, `jira.example.com`).
