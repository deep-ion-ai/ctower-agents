# Atualização do ambiente e da UI pelo Control Tower (issue #90)

Resultado do brainstorm da issue #90. As decisões estão registradas no ledger do work item da #90. A implementação está nas sub-issues da #90, com uma PR por sub-issue para a branch `feat/90-atualizacao-do-ambiente-ctowerd-ctower-e` e uma PR final para `develop`.

## 1. Decisões

| Tema | Decisão |
|---|---|
| Fonte da versão do daemon | **Build local**: um clone dedicado do repositório, `git fetch` do ref configurado e `cargo install --locked`. Não há pipeline de binários. |
| Canal | `update_ref` no `config.toml`, com padrão `main`. Aceita uma branch (ex.: `develop`) ou uma tag. |
| Atualização da UI | `tauri-plugin-updater`, com artefatos assinados e um `latest.json` na release da tag. |
| Permissão | O token de UI basta. Ele já permite `session.start`, que roda comandos no ambiente, então `env.update` não dá poder novo a quem tem o token. Não há flag extra. |
| Ordem com o hub | Nenhuma ordem obrigatória: o protocolo só ganha campos e versões minor. **Atualizar todos** atualiza o hub primeiro e os outros depois. |
| Rollback | Antes de instalar, o atualizador guarda os binários anteriores. Se o daemon novo não subir, ele volta sozinho. `ctower update --rollback` volta à mão. |
| Agendar para quando não houver sessões vivas | Fica fora desta entrega. Com sessões vivas, o daemon recusa sem `force`. |
| Plataforma | O daemon só se atualiza com systemd de usuário (Linux e WSL), igual ao `ctower restart`. |

## 2. Versão instalada e disponível

- O `build.rs` do `ctowerd` e o do `ctower` gravam o commit do build (`git rev-parse HEAD`) em `CTOWER_COMMIT`. Sem git, o valor fica vazio.
- Novas chaves do `config.toml`:
  - `update_repo`, padrão `https://github.com/deep-ion-ai/ctower-agents.git`;
  - `update_ref`, padrão `main`;
  - `update_check_interval_hours`, padrão `6` (`0` desliga a verificação periódica).
- O clone dedicado fica em `<data>/update/src`, separado do clone de desenvolvimento do usuário, e o `CARGO_TARGET_DIR` persistente fica em `<data>/update/target`, para builds incrementais.
- Verificação: o clone dedicado é criado se não existir, e depois rodam `git fetch origin <ref>`, `rev-parse FETCH_HEAD` e `git show FETCH_HEAD:Cargo.toml` (que dá a versão). Se o commit instalado for ancestral do remoto, há atualização, e a resposta inclui a lista de commits (no máximo 20). Se o instalado contiver o remoto, o ambiente está em dia ou à frente (build de desenvolvimento).
- O `env.info` ganha `commit` e `update` (`UpdateStatus`), e o protocolo passa a `1.1`. O `/healthz` passa a devolver `commit`.
- `env.check_update` verifica na hora. O daemon também verifica a cada `update_check_interval_hours`, e cada resultado novo sai no push `update.status`.

## 3. Atualizar o ambiente

```
UI ── env.update {force?} ─▶ ctowerd ── systemd-run --user --unit=ctower-update ─▶ ctower update --run
                                ▲                                                       │
                                └──── lê <data>/update/state.json e faz push ◀──────────┘
```

1. `env.update` (e `ctower update`) recusa quando há sessões vivas e não veio `force`. A mensagem diz quantas sessões são, como no `ctower restart`. Também recusa se já houver uma atualização em andamento.
2. O daemon dispara o atualizador **fora do próprio cgroup** com `systemd-run --user --unit=ctower-update --collect`. Com `KillMode=control-group`, um processo filho morreria no restart. O `systemd` impede duas unidades `ctower-update` ao mesmo tempo.
3. O atualizador (`ctower update --run`) grava cada fase em `<data>/update/state.json` e o log em `<data>/update/update.log`:
   `fetch → build → backup → install → restart → verify → done`, e em caso de erro `failed` ou `rolled_back`.
   - **build:** `cargo install --locked --path crates/{ctowerd,ctower} --root <data>/update/stage`. Se falhar, nada foi tocado e o resultado é `failed`, com o fim do log.
   - **backup:** copia os binários atuais para `<data>/update/prev/` (junto com o `prev.json`, que guarda versão e commit) e tira um snapshot do `ledger.db` (e do `hub.db`, no hub) com `VACUUM INTO` em `<data>/update/db/`. Guarda também o `user_version` de cada banco.
   - **install:** copia cada binário novo para `<bin>/.<nome>.new` e o renomeia por cima do atual, o que é atômico no mesmo diretório. `<bin>` é o diretório do executável `ctower` que está rodando.
   - **restart:** `systemctl --user restart ctowerd`.
   - **verify:** consulta o `/healthz` por até 60 s, até o `commit` ser o esperado.
   - **rollback automático** quando o verify falha: os binários de `prev/` voltam ao lugar e o daemon reinicia. Se o `user_version` de um banco tiver subido, o banco volta do snapshot. O resultado é `rolled_back`, com o motivo.
4. Enquanto vive, o daemon antigo lê o `state.json` a cada segundo e faz push de `update.status`. Depois do restart, o daemon novo lê o `state.json` no boot e o expõe em `env.info.update.last`. A UI reconecta pelo backoff do `ct-client` e confirma a versão com o `env.info`.

CLI:

- `ctower update [--force]` aplica as mesmas regras de sessões vivas, dispara a unidade e acompanha o `state.json`, imprimindo as fases.
- `ctower update --check` mostra a versão instalada e a disponível.
- `ctower update --rollback [--force]` volta para os binários de `prev/` e reinicia.

Para os testes, os comandos externos podem ser trocados por variáveis de ambiente: `CTOWER_SYSTEMCTL` (já existe), `CTOWER_SYSTEMD_RUN`, `CTOWER_CARGO` e `CTOWER_GIT`.

## 4. Tela "Atualizações" (Configurações)

- Uma linha por ambiente, com o nome, a versão e o commit atuais, a versão disponível (e os commits novos, num detalhe que se expande), o número de sessões vivas e o estado da atualização em andamento. O botão **Atualizar** pede confirmação. Com sessões vivas, a confirmação avisa que elas serão encerradas e envia `force`.
- **Verificar agora** chama `env.check_update` em todos os ambientes.
- **Atualizar todos** atualiza só os ambientes que têm atualização: o hub primeiro e os outros em seguida.
- Depois do restart, a linha mostra "reconectando…" até o `env.info` voltar com o commit novo. Depois mostra "atualizado para X" ou o erro e o fim do log.
- **Protocolo incompatível:** os métodos `env.info`, `env.check_update` e `env.update` têm nome estável para sempre, e o `ct-client` os aceita mesmo com major diferente. O aviso de incompatível passa a oferecer a atualização do lado defasado: o ambiente, se a UI for mais nova, ou a UI, se o daemon for mais novo.

## 5. Atualizar a UI

- `tauri-plugin-updater` e `tauri-plugin-process` (para reiniciar o app). A chave pública fica em `plugins.updater.pubkey` no `tauri.conf.json`, e o NSIS instala em modo `passive`.
- **Release** (tag `v*.*.*`): o job `windows` gera os artefatos do updater com `TAURI_SIGNING_PRIVATE_KEY` e `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, que vêm de secrets, usando um `--config` só na tag. O job `release` envia o `.exe` e o `.sig`, monta o `latest.json` com a URL de API de cada asset (`/releases/assets/<id>`) e o envia também.
- **Repositório privado:** a busca e o download usam `Authorization: Bearer <token>` e `Accept: application/octet-stream`. O lado Rust lê o token da distro WSL (`~/.config/ctower/github.token` ou `gh auth token`), como já faz com o token de UI. O token nunca vai para o JavaScript. Sem token, a busca é feita sem autenticação, o que funciona se o repositório ficar público.
- O endpoint do `latest.json` é descoberto em Rust: a API `releases/latest` devolve o id do asset `latest.json`.
- A tela mostra a versão da UI, a disponível com as notas, e os botões **Verificar** e **Atualizar e reiniciar** (com confirmação). A UI verifica ao abrir e depois a cada 6 h.
