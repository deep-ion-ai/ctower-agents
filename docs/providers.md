# Providers: como adicionar um tipo de conexão

Um **provider** é um tipo de serviço externo (hoje GitHub, GitLab e Jira). Uma **conexão** é um cadastro desse tipo no hub (URL ou host, fonte da credencial e opções próprias), e os projetos a referenciam. Este guia mostra onde cada parte mora e o que falta para um tipo novo. O modelo de cadastro e a operação estão em [`hub.md`](hub.md), seção 2, "Conexões".

Estado: o descritor por tipo, a conexão no hub, a migração dos cadastros antigos, o resolvedor de credencial (comando, secret e arquivo para Jira e GitLab, #274; o token do GitHub no hub e no ambiente, #275), a tela **Hub › Conexões**, a sobrescrita por conexão editável no ambiente (#276), a leitura direta da conexão no runtime, sem os campos antigos no cadastro do projeto (#277), e o descritor no lugar dos `match` de configuração e credencial por tipo, no Rust e na UI (#278), existem.

## Onde fica cada parte

| Parte | Arquivo | O que tem |
| --- | --- | --- |
| Tipos e descritores | `crates/ct-core/src/provider.rs` | `ProviderKind`, `Connection`, `CredentialSource`, `CredentialKind`, `Capabilities`, `Readiness`, o trait `ProviderDescriptor`, os descritores `GITHUB`, `GITLAB`, `JIRA`, o registro `PROVIDERS`, `descriptor(kind)`, `provider_infos` (o que vai à UI), `Role` e `check_role` |
| Lados do projeto | `crates/ct-core/src/provider.rs` (`project_sides`, `project_ref`, `connection_of`, `project_connection`, `uses_connection`, `share_connection`, `validate_project`) e `crates/ct-core/src/tracker.rs` (`TrackerConfig::provider`/`connection`, `ForgeConfig::provider`/`connection`) | o rastreador e a forja como pares (tipo, conexão referenciada), sem `match` por tipo: qual conexão o projeto usa (a referenciada ou a padrão do tipo), quem usa uma conexão, a validação de cada lado pelo descritor |
| Cadastro da conexão no hub | `crates/ct-hub-store/src/connection.rs` | `add`, `update`, `delete` (recusa conexão em uso), `test` (sem rede), `check_project_connections`, `env_connections` (o que vai no `GET /sync/config`) |
| RPC | `crates/cthubd/src/ws.rs` (`connections_call`) e `crates/cthubd/src/methods.rs` | `hub.connections.list|add|update|delete|test`. O cadastro do projeto (`provider_param`) normaliza cada lado pelo descritor (`normalize_tracker`, `normalize_forge`) |
| Referência no projeto | `crates/ct-core/src/tracker.rs` | `JiraConfig.connection`, `GitlabConfig.connection` e `GithubConfig.connection` (`Option<i64>`; o GitHub usa a mesma no rastreador e na forja, `share_connection`). Jira e GitLab exigem a conexão (`validate_tracker`/`validate_forge` do descritor) |
| Leitura da conexão | `crates/ct-core/src/provider.rs` (`resolve_connections`, `ResolvedConnections`, `ProjectEndpoints`) | o ponto único de leitura dos tipos com prefixo de sessão: dá a URL do Jira, o host do GitLab, o `config_file`, os prefixos da sessão (`prefix(kind)`) e os `endpoints` da UI a partir das conexões e da fonte de quem lê. O hub usa `Hub::project_connections` e `Hub::with_endpoints` (`crates/ct-hub-store/src/connection.rs`); o ambiente, `SharedCredentials::resolve` (`crates/ct-core/src/credentials.rs`) e `Sessions::project_connections` |
| Migração dos cadastros antigos | `crates/ct-hub-store/src/legacy_connections.rs` | cria as conexões a partir de `url`/`host` e `credential_cmd`, idempotente, com cópia do `hub.db` antes |
| Conexões no ambiente | `crates/ct-ledger/src/hub_config.rs` (`hub_connection_cache`, `connection_warnings`) e `crates/ct-server/src/hub_config.rs` (`apply_connections`) | no pull, guarda as conexões no cache, põe em uso no `SharedCredentials` e avisa o que falta (projeto sem conexão, conexão ausente, fonte sem comando, sobrescrita órfã). O cadastro do projeto não muda |
| `[connections]` do `config.toml` | `crates/ct-server/src/env_config.rs` e `crates/ct-core/src/credentials.rs` | a sobrescrita do comando por conexão (`ConnectionOverrides::source`); `env.connections.get|set` (`connection_rows`, `EnvConfig::set_connection`); `convert_credentials_file` converte `[credentials]`; `migrate_github_token` leva o `github.token` do ambiente para a sobrescrita da conexão GitHub |
| Fonte efetiva no ambiente | `crates/ct-core/src/credentials.rs` (`SharedCredentials::source`) e `crates/ct-sessions/src/lib.rs` (`Sessions::source`, `Sessions::sources`) | a fonte de cada tipo que o projeto usa: `[credentials]` do projeto (legado, só Jira e GitLab), a conexão do hub (a referenciada ou a padrão do tipo) com a sobrescrita, ou a fonte de reserva do descritor (`fallback_credential`); o pull e o boot atualizam as conexões em uso (`set_connections`) |
| Resolvedor de credencial | `crates/ct-proc/src/resolver.rs` | `CredentialResolver` (cache em memória), `Ask`, `resolve()` com um braço por `CredentialSource`. O ambiente (`crates/ct-sessions/src/resolver.rs`, com a chave do cache `cache_key(tipo, projeto)`) e o hub (`crates/cthubd/src/github.rs`) usam o mesmo |
| Token do GitHub | `crates/ct-gh/src/lib.rs` (`TokenProvider`, `GhClient::with_provider`), `crates/ct-sessions/src/github.rs` (`GithubSync::client_for`) e `crates/cthubd/src/github.rs` (`HubGithub::client_for`) | o cliente do GitHub de cada projeto pede o token ao resolvedor, com a fonte da conexão `github` (#275) |
| UI | `ui/src/hub/providers.ts` (registro), `ui/src/protocol/gen/providers.ts` (gerado), `ui/src/hub/connections.ts`, `ui/src/hub/ConnectionsPanel.tsx`, `ui/src/project/settings/TrackerForm.tsx` e `ui/src/settings/ConnectionOverridesSettings.tsx` | o registro dos tipos (dados do descritor mais os textos do formulário), tela Conexões, escolha da conexão no projeto, sobrescrita no ambiente (Configurações › Ambiente › Credenciais das conexões). As telas consultam o registro (`providerOf`) e não comparam tipos |

## Os cinco elementos de uma conexão

- **`ProviderKind`**: o tipo (`github`, `gitlab`, `jira`). Vai no fio em minúsculas.
- **`endpoint`**: URL (Jira), host (GitLab) ou API (GitHub, vazio = o padrão do tipo, `default_endpoint`).
- **`CredentialSource`**: de onde vem o token. As variantes são `secret` (nome de um secret do hub), `command` (a primeira linha da saída é o token), `cli` (`gh auth token`), `file` (caminho) e `none`. Ela é serializada com `kind` na tag, por exemplo `{ "kind": "command", "cmd": "pass show jira/token | head -1" }`. A forma, sem o valor, é `CredentialKind` (`CredentialSource::kind`).
- **`options`**: específico do tipo, validado pelo descritor (o Jira usa `config_file`).
- **`Capabilities`**: o que o tipo serve, `tracker` (issues) e/ou `forge` (repositório, PRs e MRs). É isso que `check_role` confere quando um projeto referencia a conexão.

## O descritor

`ProviderDescriptor` leva tudo o que é configuração e credencial de um tipo. Os métodos com implementação padrão só precisam ser escritos quando o tipo foge do padrão.

| Método | Para quê | Padrão |
| --- | --- | --- |
| `kind`, `label`, `capabilities` | tipo, nome nas mensagens e na UI, papéis que serve | (obrigatórios) |
| `token_env` | variável que leva o token no prefixo da sessão e na CLI do tipo (`JIRA_API_TOKEN`, `GITLAB_TOKEN`, `GH_TOKEN`) | (obrigatório) |
| `credential_kinds` | formas de credencial aceitas, na ordem do formulário. Vai à UI (`PROVIDER_INFOS`), que não repete a regra | (obrigatório) |
| `validate` | confere a conexão sem rede. Os auxiliares `check_kind`, `check_options` e `check_token_source` (que usa `credential_kinds`) servem de base | (obrigatório) |
| `default_endpoint` | endpoint quando a conexão não diz outro | nenhum |
| `default_connection` | conexão que vale para o projeto que não referencia uma (a `github`) | nenhuma: o projeto precisa referenciar |
| `fallback_credential` | fonte do token quando a conexão do projeto não veio ao ambiente | `None` (o resolvedor recusa com o motivo); o GitHub usa `Cli` |
| `session_cli`, `session_env` | a CLI que a sessão chama com o prefixo (`jira`, `glab`) e as variáveis antes do token (`JIRA_CONFIG_FILE`, `GITLAB_HOST`) | sem prefixo (o tipo fica fora de `resolve_connections`) |
| `session_prefix` | o prefixo das instruções da sessão: `session_env`, `<token_env>="$(<comando>)"` e `session_cli` | montado dos três acima |
| `readiness` | prontidão estática (`hub.connections.test`) | pelo `validate` |
| `validate_tracker`, `validate_forge`, `validate_sync` | confere o lado do projeto que usa o tipo (o Jira exige conexão, chave e campos; o filtro do sync) | nada a conferir |
| `normalize_tracker`, `normalize_forge` | normaliza o lado do projeto no cadastro (chave em maiúsculas, caminho do GitLab pela URL de clone) | nada muda |

O comando do token no prefixo vem de `source_cmd`: o próprio comando, `ctower secret get NOME` para o `secret` e a leitura da primeira linha não vazia do arquivo (`file_cmd`, com `~` como `"$HOME"`) para o `file`, o mesmo token que o resolvedor lê.

## Passo a passo para um tipo novo

Use o Jira como modelo: `JiraDescriptor` em `provider.rs`. Para um tipo de conexão (cadastro no hub, credencial no ambiente, tela de Conexões e sobrescrita), bastam os passos 1 a 4. Os passos 5 e 6 são do projeto que usa o tipo.

1. **Tipo.** Acrescente a variante em `ProviderKind`, pela macro `str_enum!`, com o nome do fio. A tabela `connection` guarda `kind` como `TEXT` sem `CHECK`, então o hub não precisa de migração para o tipo novo. A validação é do descritor.

2. **Descritor.** Implemente `ProviderDescriptor` para o tipo (a tabela acima): os quatro métodos obrigatórios e os que fogem do padrão. Inclua o descritor como `static` e na lista `PROVIDERS`, na posição em que ele deve aparecer no formulário. `descriptor(kind)` falha se o tipo não estiver ali, e o teste `registro_tem_um_descritor_por_tipo` pega isso. A partir daqui, sem mais código:
   - o hub cadastra, valida, testa e confere o papel da conexão (`hub.connections.*`, `check_role`, `check_project_connections`);
   - o ambiente acha a fonte do token (`SharedCredentials::source`), com a sobrescrita `[connections."nome"]` e a fonte de reserva do descritor; a chave do cache é `<tipo>:<projeto>` (`cache_key`); o mascaramento das sessões cobre o token (`connection_mask` percorre `Sessions::sources`); a sobrescrita editada limpa o cache dos projetos que usam a conexão (`invalidate_connection`, por `uses_connection`);
   - a tela Configurações › Ambiente › Credenciais das conexões lista a conexão e os projetos que a usam (`connection_rows`);
   - com `session_cli`, o tipo entra em `resolve_connections`: os avisos do pull (projeto sem conexão, conexão ausente, fonte sem comando) e o prefixo (`ResolvedConnections::prefix`).

3. **Tipos TypeScript.** Regenere os tipos e os dados dos descritores que a UI importa:
   ```
   cargo test -p ct-server --features ts export_ts
   cargo test -p cthubd --features ts export_ts
   ```
   Depois versione `ui/src/protocol/gen` (inclusive `providers.ts`, com `PROVIDER_INFOS`) e `ui/src/protocol/hub-gen`. O CI (job "Tipos TypeScript da UI em dia", em `.github/workflows/ci.yml`) falha se o gerado não bater com o Rust.

4. **UI.** Em `ui/src/hub/providers.ts`, acrescente o tipo a `UI` (o TypeScript recusa o registro sem ele): `endpointLabel` (URL, host ou API), `endpointHint` e `cmdHint` (exemplos fictícios), `cliTokenCmd` se a CLI do tipo imprime o token, e `options`, os campos próprios do formulário (gravados em `options` da conexão). Rótulo, formas de credencial aceitas, endpoint padrão e conexão padrão vêm do descritor. A tela Conexões, o seletor de conexão do projeto (`ConnectionSelect` em `TrackerForm.tsx`, que lista pelo tipo com `connectionsOfKind` e deixa a escolha opcional quando há `default_connection`) e a sobrescrita do ambiente leem só o registro.

5. **Projeto que usa o tipo.** Para o projeto referenciar a conexão como rastreador ou forja, o tipo precisa de uma variante em `TrackerKind`/`TrackerConfig` ou `ForgeKind`/`ForgeConfig` (`crates/ct-core/src/tracker.rs`), com a configuração própria do projeto e o campo `connection`, sem endereço nem credencial. Os braços de `connection`/`connection_mut`, `config_json` e `from_columns` são a configuração; `item_ref`, `item_url`, `issue_view_cmd`, o padrão de branch (`branch.rs`), os templates (`session_templates.rs`), o sync, o checkout e a prontidão do ambiente (`TrackerReadiness`) são comportamento e ficam com as traits `Tracker`/`Forge` do épico seguinte. A combinação de rastreador e forja aceita está em `supported_combination`. Na UI, o formulário do projeto do tipo (`GeneralForm.tsx`, `NewProjectDialog.tsx`) é a UI dele.

6. **Fonte de credencial nova.** As cinco variantes de `CredentialSource` já existem. Uma variante nova exige um braço em `resolve()` (`crates/ct-proc/src/resolver.rs`) e em `source_cmd` (`provider.rs`), que devem dar o mesmo token, em `CredentialSource::kind`/`CredentialKind`, o rótulo da UI (`CRED_TEXT` em `providers.ts`) e a regeneração do passo 3.

## O que ainda não existe

- **Token de Jira e GitLab no hub.** O hub só resolve o token do GitHub (#275). Ele não fala com o Jira nem com o GitLab e só monta prefixos de sessão (o `issue_view` do gestor, as telas), com o comando de `source_cmd`. O `secret` desses prefixos resolve na sessão, no ambiente (`ctower secret get`).
- **Comportamento por tipo.** Sync, leitura de issue, branches, links e o checkout ainda usam `match` por tipo, assim como a lista de `ResolvedConnections` que cada lado lê (`jira_url`, `gitlab_host`, `jira_config_file`) e os campos de `ProjectEndpoints` (`{{jira}}`, `{{glab}}` dos templates). O descritor cobre configuração e credencial, não o comportamento.
- **Legado.** `[credentials."owner/repo"]` (`legacy_cmd`) e a migração (`legacy_connections.rs`, `convert_credentials_file`) só conhecem o Jira e o GitLab, e o `connection_users`/`env_connections` do hub acham a conexão GitHub padrão em SQL. Um tipo novo não passa por eles.

Nenhum exemplo deste guia usa dado real: os nomes são fictícios (`jira-empresa`, `jira.example.com`).
