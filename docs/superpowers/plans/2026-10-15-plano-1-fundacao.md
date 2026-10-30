# Control Tower — Plano 1: Fundação — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provisionar o ambiente WSL dedicado e entregar as bibliotecas de base do daemon (modelo de domínio, ledger SQLite, worktrees git) e o CLI `ctower hook` com guarda-corpo de branch protegida e spool offline.

**Architecture:** Workspace Cargo com crates de responsabilidade única: `ct-core` (tipos, máquinas de estado, nomes de branch, guarda-corpo — sem I/O), `ct-ledger` (persistência SQLite síncrona via `rusqlite`), `ct-git` (worktrees via `git` CLI) e o binário `ctower` (hooks do Claude Code). Nada é assíncrono neste plano; o servidor/daemon vem no Plano 2.

**Tech Stack:** Rust stable (edição 2021), `rusqlite` 0.32 (bundled), `serde`/`serde_json`, `thiserror` 2, `uuid` 1, `clap` 4, `ureq` 2; testes com `tempfile`, `assert_cmd`, `predicates`. Ubuntu 24.04 em WSL2 com systemd.

**Spec:** `docs/superpowers/specs/2026-10-15-control-tower-design.md` (roadmap em `docs/superpowers/plans/2026-10-15-control-tower-roadmap.md`)

## Global Constraints

- Daemon e CLI rodam apenas em Unix (Linux/macOS); nada de código específico de Windows nos crates.
- Branches protegidas padrão: `["develop","main","master"]`; branch base padrão: `develop`.
- Branch de épico: `wip/<n>-<slug>`; issue avulsa: `feat/<n>-<slug>` (`fix` com label `bug`, `chore` com label `chore`).
- Status de work item (strings exatas): `backlog`, `em_execucao`, `aguardando_voce`, `em_revisao`, `concluido`, `bloqueado`, `pausado`.
- Estados de sessão (strings exatas): `starting`, `running`, `idle`, `exited`, `interrupted`, `archived`.
- Origem de decisão: `autonoma` (pendente de revisão), `orientada` e `manual` (nascem revisadas).
- `event` é append-only; `decision` só altera `reviewed_at`/`reviewed_in_pr`; nada é apagado.
- Hooks nunca bloqueiam o agente por falha de comunicação: saem com 0. Única exceção: guarda-corpo (`pre-tool-use`) sai com 2 ao bloquear.
- Hook sem `CTOWER_SESSION` definido sai com 0 imediatamente, sem efeito algum.
- Variáveis de ambiente das sessões: `CTOWER_SESSION`, `CTOWER_URL`, `CTOWER_TOKEN`, `CTOWER_PROTECTED` (lista separada por vírgula).
- Diretórios XDG: dados em `~/.local/share/ctower/` (spool em `hooks-spool/`), config em `~/.config/ctower/`.
- Projetos e worktrees no ext4 da distro, nunca em `/mnt/*`.
- Timestamps em milissegundos Unix (`i64`).
- Idioma de mensagens ao usuário/agente: PT-BR.
- Nunca commitar em `develop`; trabalho na branch `feat/plano-1-fundacao` com PR para `develop`.

## Review Focus

1. **Operadores dentro de aspas no guarda-corpo** — `git commit -m "fix && git push origin develop"` numa branch `wip/*` deve ser permitido (o texto é mensagem, não comando). Teste na Task 4.
2. **Títulos com acentos, emoji e pontuação** — o nome de branch gerado deve conter só `[a-z0-9-/]` e nunca terminar em `-`. Teste na Task 3.
3. **Hook com stdin inválido ou daemon fora do ar** — deve sair com 0 e, quando houver o que entregar, gravar no spool. Testes na Task 8.
4. **Boot repetido do daemon** — `mark_interrupted` chamado duas vezes não deve duplicar eventos nem tocar sessões `exited`/`archived`, e deve preservar o horário do último sinal. Teste na Task 6.
5. **Caminho de worktree ocupado por outra branch** — erro explícito, sem alterar nada. Teste na Task 7.

---

## File Structure

```
control-tower/
├─ Cargo.toml                         # workspace
├─ rust-toolchain.toml
├─ .github/workflows/ci.yml
├─ scripts/provision/
│  ├─ create-distro.ps1               # lado Windows: cria a distro
│  └─ provision.sh                    # lado Linux: ferramentas (idempotente)
└─ crates/
   ├─ ct-core/src/
   │  ├─ lib.rs
   │  ├─ clock.rs                     # now_ms()
   │  ├─ model.rs                     # enums, máquinas de estado, gatilhos
   │  ├─ branch.rs                    # slugify, branch_for, worktree_dir_name
   │  └─ guard/
   │     ├─ mod.rs                    # check_command, GuardContext, Verdict
   │     └─ tokenize.rs               # split_segments
   ├─ ct-ledger/src/
   │  ├─ lib.rs                       # Ledger, open, helpers, test_support
   │  ├─ error.rs
   │  ├─ schema.rs                    # migrações
   │  ├─ projects.rs
   │  ├─ work_items.rs
   │  ├─ events.rs
   │  ├─ sessions.rs
   │  └─ decisions.rs
   ├─ ct-git/src/lib.rs               # ensure_worktree, current_branch, remove_worktree
   └─ ctower/
      ├─ src/main.rs
      ├─ src/hook.rs
      └─ tests/hook_cli.rs
```

---

### Task 1: Provisionar a distro WSL dedicada e mover o repositório

**Files:**
- Create: `scripts/provision/create-distro.ps1`
- Create: `scripts/provision/provision.sh`

**Interfaces:**
- Consumes: nada.
- Produces: distro `control-tower` com systemd, git, gh, Rust, Node, Claude Code; repositório clonado em `~/projetos/control-tower` dentro dela. Todas as tasks seguintes rodam **dentro da distro**.

> Este task é executado no Windows (passo 1–3) e depois dentro da distro. Os passos manuais de login (`gh auth login`, `claude`) exigem o humano.

- [ ] **Step 1: Criar `scripts/provision/create-distro.ps1`** (no clone atual em `E:\projetos\estudos\control-tower`, branch `docs/spec-inicial` ou nova branch `chore/provisionamento` a partir dela)

```powershell
<#
.SYNOPSIS
  Cria a distro WSL dedicada do Control Tower a partir do rootfs oficial do Ubuntu 24.04.
.EXAMPLE
  .\create-distro.ps1 -User usuario
#>
param(
    [Parameter(Mandatory = $true)][string]$User,
    [string]$Name = "control-tower",
    [string]$InstallDir = "$env:LOCALAPPDATA\wsl\control-tower",
    [string]$RootfsUrl = "https://cloud-images.ubuntu.com/wsl/releases/24.04/current/ubuntu-noble-wsl-amd64-wsl.rootfs.tar.gz"
)
$ErrorActionPreference = "Stop"

$existing = (wsl.exe -l -q) -replace "`0", "" | Where-Object { $_.Trim() -eq $Name }
if ($existing) {
    Write-Host "Distro '$Name' já existe; nada a fazer."
    exit 0
}

New-Item -ItemType Directory -Force $InstallDir | Out-Null
$tar = Join-Path $env:TEMP "control-tower-rootfs.tar.gz"
if (-not (Test-Path $tar)) {
    Write-Host "Baixando rootfs do Ubuntu 24.04..."
    Invoke-WebRequest -Uri $RootfsUrl -OutFile $tar
}

wsl.exe --import $Name $InstallDir $tar --version 2
if ($LASTEXITCODE -ne 0) { throw "wsl --import falhou" }

$setup = @"
set -e
id -u $User >/dev/null 2>&1 || useradd -m -s /bin/bash -G sudo $User
echo '$User ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/90-$User
chmod 440 /etc/sudoers.d/90-$User
printf '[boot]\nsystemd=true\n[user]\ndefault=$User\n' > /etc/wsl.conf
"@
wsl.exe -d $Name -u root -- bash -c $setup
if ($LASTEXITCODE -ne 0) { throw "configuração inicial falhou" }

wsl.exe --terminate $Name
Write-Host "Distro '$Name' criada. Próximo passo: wsl -d $Name e rodar scripts/provision/provision.sh"
```

- [ ] **Step 2: Criar `scripts/provision/provision.sh`**

```bash
#!/usr/bin/env bash
# Provisiona a distro dedicada do Control Tower. Idempotente: pode rodar várias vezes.
set -euo pipefail

if [[ $EUID -eq 0 ]]; then
  echo "Rode como usuário comum (o script usa sudo quando precisa)." >&2
  exit 1
fi

sudo apt-get update -y
sudo apt-get install -y build-essential pkg-config git curl ca-certificates unzip jq

if ! command -v gh >/dev/null; then
  sudo mkdir -p -m 755 /etc/apt/keyrings
  curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | sudo tee /etc/apt/keyrings/githubcli-archive-keyring.gpg >/dev/null
  sudo chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | sudo tee /etc/apt/sources.list.d/github-cli.list >/dev/null
  sudo apt-get update -y
  sudo apt-get install -y gh
fi

if [[ ! -x "$HOME/.cargo/bin/cargo" ]]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal -c clippy -c rustfmt
fi

if ! command -v node >/dev/null; then
  curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash -
  sudo apt-get install -y nodejs
fi

if ! command -v claude >/dev/null && [[ ! -x "$HOME/.local/bin/claude" ]]; then
  curl -fsSL https://claude.ai/install.sh | bash
fi

mkdir -p "$HOME/projetos" "$HOME/worktrees" "$HOME/.local/share/ctower" "$HOME/.config/ctower"

echo
echo "Provisionamento concluído. Passos manuais restantes:"
echo "  1. gh auth login && gh auth setup-git"
echo "  2. claude   (fazer login)"
echo "  3. git config --global user.name / user.email"
```

- [ ] **Step 3: Criar a distro (humano, PowerShell no Windows)**

Run: `powershell -ExecutionPolicy Bypass -File scripts\provision\create-distro.ps1 -User <seu-usuario>`
Expected: `Distro 'control-tower' criada.` e `wsl -l -v` lista `control-tower` com VERSION 2.

- [ ] **Step 4: Provisionar (dentro da distro)**

Run (Windows): `wsl -d control-tower -- bash /mnt/e/projetos/estudos/control-tower/scripts/provision/provision.sh`
Expected: termina com `Provisionamento concluído.` Depois o humano executa os 3 passos manuais listados.

- [ ] **Step 5: Verificar o ambiente**

Run: `wsl -d control-tower -- bash -lc 'systemctl is-system-running; cargo --version; gh auth status; claude --version; findmnt -n -o FSTYPE -T ~'`
Expected: `running` ou `degraded`; versões impressas; `gh` logado; `ext4`.

- [ ] **Step 6: Commitar os scripts e mover o repositório para a distro**

No clone do Windows:

```bash
git checkout -b chore/provisionamento origin/develop   # após o PR #1 mesclado; senão, a partir de docs/spec-inicial
git add scripts/provision
git commit -m "chore: scripts de provisionamento da distro WSL dedicada"
git push -u origin chore/provisionamento
gh pr create --base develop --title "chore: scripts de provisionamento da distro WSL" --body "Scripts da Task 1 do Plano 1."
```

Dentro da distro:

```bash
cd ~/projetos && git clone https://github.com/deep-ion-ai/ctower-agents.git && cd control-tower
git checkout -b feat/plano-1-fundacao origin/develop
```

Expected: `git status` limpo em `~/projetos/control-tower`, branch `feat/plano-1-fundacao`. A partir daqui, **todo trabalho acontece na distro**; o clone em `E:\` pode ser removido depois que os PRs estiverem mesclados.

---

### Task 2: Workspace Cargo, `ct-core::model` e CI

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`
- Create: `crates/ct-core/Cargo.toml`, `crates/ct-core/src/lib.rs`, `crates/ct-core/src/clock.rs`, `crates/ct-core/src/model.rs`
- Test: testes unitários em `crates/ct-core/src/model.rs`

**Interfaces:**
- Consumes: nada.
- Produces:
  - `ct_core::now_ms() -> i64`
  - `ct_core::ParseEnumError { kind: &'static str, value: String }`
  - enums (todos `Copy + Eq + Hash + Serialize + Deserialize + Display + FromStr<Err = ParseEnumError>`, com `as_str(self) -> &'static str` e `const ALL: &[Self]`): `WorkItemKind { Epic, Issue }`, `WorkItemStatus { Backlog, EmExecucao, AguardandoVoce, EmRevisao, Concluido, Bloqueado, Pausado }`, `SessionState { Starting, Running, Idle, Exited, Interrupted, Archived }`, `Tool { Claude, Codex }`, `DecisionOrigin { Autonoma, Orientada, Manual }`
  - `SessionState::is_live(self) -> bool`, `SessionState::can_transition_to(self, next: SessionState) -> bool`
  - `WorkItemTrigger { SessionActive, NeedsHuman, PrOpened, PrMerged }` e `ct_core::next_status(current: WorkItemStatus, trigger: WorkItemTrigger) -> Option<WorkItemStatus>`

- [ ] **Step 1: Criar o workspace**

`Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
ct-core = { path = "crates/ct-core" }
ct-ledger = { path = "crates/ct-ledger" }
ct-git = { path = "crates/ct-git" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
rusqlite = { version = "0.32", features = ["bundled"] }
uuid = { version = "1", features = ["v4"] }
clap = { version = "4", features = ["derive"] }
ureq = { version = "2", default-features = false }
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["clippy", "rustfmt"]
```

`crates/ct-core/Cargo.toml`:

```toml
[package]
name = "ct-core"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

`crates/ct-core/src/lib.rs`:

```rust
//! Domínio do Control Tower: tipos, máquinas de estado e regras puras (sem I/O).

mod clock;
mod model;

pub use clock::now_ms;
pub use model::*;
```

`crates/ct-core/src/clock.rs`:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

/// Milissegundos desde a época Unix.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 2: Escrever os testes de `model.rs` (falhando)**

`crates/ct-core/src/model.rs` (por enquanto só os testes):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_ida_e_volta_por_string() {
        for v in WorkItemKind::ALL {
            assert_eq!(v.as_str().parse::<WorkItemKind>().unwrap(), *v);
        }
        for v in WorkItemStatus::ALL {
            assert_eq!(v.as_str().parse::<WorkItemStatus>().unwrap(), *v);
        }
        for v in SessionState::ALL {
            assert_eq!(v.as_str().parse::<SessionState>().unwrap(), *v);
        }
        for v in Tool::ALL {
            assert_eq!(v.as_str().parse::<Tool>().unwrap(), *v);
        }
        for v in DecisionOrigin::ALL {
            assert_eq!(v.as_str().parse::<DecisionOrigin>().unwrap(), *v);
        }
    }

    #[test]
    fn string_invalida_gera_erro_com_contexto() {
        let err = "xpto".parse::<Tool>().unwrap_err();
        assert_eq!(err.kind, "tool");
        assert_eq!(err.value, "xpto");
    }

    #[test]
    fn serde_usa_as_mesmas_strings() {
        assert_eq!(
            serde_json::to_string(&WorkItemStatus::AguardandoVoce).unwrap(),
            "\"aguardando_voce\""
        );
        let s: SessionState = serde_json::from_str("\"interrupted\"").unwrap();
        assert_eq!(s, SessionState::Interrupted);
    }

    #[test]
    fn sessao_transicoes_validas() {
        use SessionState::*;
        assert!(Starting.can_transition_to(Running));
        assert!(Starting.can_transition_to(Idle));
        assert!(Running.can_transition_to(Idle));
        assert!(Idle.can_transition_to(Running));
        assert!(Running.can_transition_to(Exited));
        assert!(Idle.can_transition_to(Interrupted));
        assert!(Interrupted.can_transition_to(Archived));
        assert!(Exited.can_transition_to(Archived));
    }

    #[test]
    fn sessao_transicoes_invalidas() {
        use SessionState::*;
        assert!(!Exited.can_transition_to(Running));
        assert!(!Archived.can_transition_to(Running));
        assert!(!Interrupted.can_transition_to(Running));
        assert!(!Running.can_transition_to(Starting));
        assert!(!Running.can_transition_to(Archived));
    }

    #[test]
    fn sessao_viva() {
        use SessionState::*;
        assert!(Starting.is_live() && Running.is_live() && Idle.is_live());
        assert!(!Exited.is_live() && !Interrupted.is_live() && !Archived.is_live());
    }

    #[test]
    fn gatilhos_definem_status() {
        use WorkItemStatus::*;
        assert_eq!(next_status(Backlog, WorkItemTrigger::SessionActive), Some(EmExecucao));
        assert_eq!(next_status(EmExecucao, WorkItemTrigger::NeedsHuman), Some(AguardandoVoce));
        assert_eq!(next_status(AguardandoVoce, WorkItemTrigger::SessionActive), Some(EmExecucao));
        assert_eq!(next_status(EmExecucao, WorkItemTrigger::PrOpened), Some(EmRevisao));
        assert_eq!(next_status(EmRevisao, WorkItemTrigger::PrMerged), Some(Concluido));
    }

    #[test]
    fn gatilho_sem_mudanca_retorna_none() {
        use WorkItemStatus::*;
        assert_eq!(next_status(EmExecucao, WorkItemTrigger::SessionActive), None);
    }

    #[test]
    fn gatilho_vence_status_manual() {
        use WorkItemStatus::*;
        assert_eq!(next_status(Pausado, WorkItemTrigger::SessionActive), Some(EmExecucao));
        assert_eq!(next_status(Bloqueado, WorkItemTrigger::NeedsHuman), Some(AguardandoVoce));
    }

    #[test]
    fn concluido_so_reabre_com_atividade_de_sessao() {
        use WorkItemStatus::*;
        assert_eq!(next_status(Concluido, WorkItemTrigger::PrOpened), None);
        assert_eq!(next_status(Concluido, WorkItemTrigger::NeedsHuman), None);
        assert_eq!(next_status(Concluido, WorkItemTrigger::SessionActive), Some(EmExecucao));
    }
}
```

- [ ] **Step 3: Rodar e ver falhar**

Run: `cargo test -p ct-core`
Expected: FAIL de compilação (`cannot find type WorkItemKind`, etc.).

- [ ] **Step 4: Implementar `model.rs`** (acima do bloco de testes)

```rust
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("valor inválido para {kind}: {value}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}

/// Enum persistido como string. Mantém `as_str`, `Display`, `FromStr` e serde alinhados.
macro_rules! str_enum {
    ($name:ident, $kind:literal, { $($variant:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( #[serde(rename = $s)] $variant ),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok($name::$variant),)+
                    _ => Err(ParseEnumError { kind: $kind, value: s.to_string() }),
                }
            }
        }
    };
}

str_enum!(WorkItemKind, "work_item_kind", { Epic => "epic", Issue => "issue" });

str_enum!(WorkItemStatus, "work_item_status", {
    Backlog => "backlog",
    EmExecucao => "em_execucao",
    AguardandoVoce => "aguardando_voce",
    EmRevisao => "em_revisao",
    Concluido => "concluido",
    Bloqueado => "bloqueado",
    Pausado => "pausado",
});

str_enum!(SessionState, "session_state", {
    Starting => "starting",
    Running => "running",
    Idle => "idle",
    Exited => "exited",
    Interrupted => "interrupted",
    Archived => "archived",
});

str_enum!(Tool, "tool", { Claude => "claude", Codex => "codex" });

str_enum!(DecisionOrigin, "decision_origin", {
    Autonoma => "autonoma",
    Orientada => "orientada",
    Manual => "manual",
});

impl SessionState {
    /// Sessão com processo (supostamente) vivo.
    pub fn is_live(self) -> bool {
        matches!(self, SessionState::Starting | SessionState::Running | SessionState::Idle)
    }

    pub fn can_transition_to(self, next: SessionState) -> bool {
        use SessionState::*;
        matches!(
            (self, next),
            (Starting, Running | Idle | Exited | Interrupted)
                | (Running, Idle | Exited | Interrupted)
                | (Idle, Running | Exited | Interrupted)
                | (Interrupted, Archived)
                | (Exited, Archived)
        )
    }
}

/// Eventos automáticos que movem o status de um work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemTrigger {
    SessionActive,
    NeedsHuman,
    PrOpened,
    PrMerged,
}

/// Novo status para um gatilho automático, ou `None` se nada muda.
///
/// O gatilho vence status manuais (`bloqueado`, `pausado`); um item `concluido`
/// só é reaberto por atividade de sessão.
pub fn next_status(current: WorkItemStatus, trigger: WorkItemTrigger) -> Option<WorkItemStatus> {
    let target = match trigger {
        WorkItemTrigger::SessionActive => WorkItemStatus::EmExecucao,
        WorkItemTrigger::NeedsHuman => WorkItemStatus::AguardandoVoce,
        WorkItemTrigger::PrOpened => WorkItemStatus::EmRevisao,
        WorkItemTrigger::PrMerged => WorkItemStatus::Concluido,
    };
    if current == WorkItemStatus::Concluido && trigger != WorkItemTrigger::SessionActive {
        return None;
    }
    (target != current).then_some(target)
}
```

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test -p ct-core`
Expected: PASS (10 testes).

- [ ] **Step 6: Adicionar CI**

`.github/workflows/ci.yml`:

```yaml
name: ci

on:
  push:
    branches: [develop, main]
  pull_request:

jobs:
  rust:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: sem saída de erro.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml rust-toolchain.toml .github crates/ct-core
git commit -m "feat(core): workspace, modelo de domínio e máquinas de estado"
```

---

### Task 3: `ct-core::branch` — nomes de branch e worktree

**Files:**
- Create: `crates/ct-core/src/branch.rs`
- Modify: `crates/ct-core/src/lib.rs` (adicionar `pub mod branch;`)
- Test: testes unitários em `branch.rs`

**Interfaces:**
- Consumes: `WorkItemKind` (Task 2).
- Produces:
  - `ct_core::branch::slugify(title: &str, max_len: usize) -> String`
  - `ct_core::branch::branch_for(kind: WorkItemKind, number: u64, title: &str, labels: &[String]) -> String`
  - `ct_core::branch::worktree_dir_name(branch: &str) -> String`

- [ ] **Step 1: Escrever os testes (falhando)**

`crates/ct-core/src/branch.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkItemKind;

    fn labels(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn slug_remove_acentos_e_pontuacao() {
        assert_eq!(
            slugify("Épico: Autenticação OAuth 2.0!", 60),
            "epico-autenticacao-oauth-2-0"
        );
    }

    #[test]
    fn slug_lida_com_emoji_e_espacos() {
        assert_eq!(slugify("  🚀 Lançar   versão  ", 60), "lancar-versao");
        assert_eq!(slugify("  --  ", 60), "");
    }

    #[test]
    fn slug_trunca_sem_hifen_no_fim() {
        let s = slugify("aaaa bbbb cccc", 6);
        assert_eq!(s, "aaaa-b");
        let s = slugify("aaaa bbbb", 5);
        assert_eq!(s, "aaaa");
    }

    #[test]
    fn slug_so_tem_caracteres_seguros() {
        let s = slugify("Ação/çõ \"aspas\" <tag> ~^:?*[]\\ ünïcödé ß", 80);
        assert!(s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{s}");
        assert!(!s.starts_with('-') && !s.ends_with('-'));
        assert!(!s.contains("--"));
    }

    #[test]
    fn branch_de_epico_usa_wip() {
        assert_eq!(
            branch_for(WorkItemKind::Epic, 12, "Autenticação", &[]),
            "wip/12-autenticacao"
        );
    }

    #[test]
    fn branch_de_issue_por_label() {
        assert_eq!(branch_for(WorkItemKind::Issue, 7, "Corrige login", &labels(&["Bug"])), "fix/7-corrige-login");
        assert_eq!(branch_for(WorkItemKind::Issue, 8, "Limpa deps", &labels(&["chore"])), "chore/8-limpa-deps");
        assert_eq!(branch_for(WorkItemKind::Issue, 9, "Export CSV", &labels(&["enhancement"])), "feat/9-export-csv");
    }

    #[test]
    fn branch_sem_slug_usa_so_numero() {
        assert_eq!(branch_for(WorkItemKind::Issue, 5, "🚀🚀", &[]), "feat/5");
    }

    #[test]
    fn diretorio_de_worktree_nao_tem_barra() {
        assert_eq!(worktree_dir_name("wip/12-auth"), "wip__12-auth");
    }
}
```

Adicionar em `lib.rs`: `pub mod branch;`

- [ ] **Step 2: Rodar e ver falhar**

Run: `cargo test -p ct-core branch`
Expected: FAIL de compilação (`cannot find function slugify`).

- [ ] **Step 3: Implementar** (acima dos testes em `branch.rs`)

```rust
//! Convenções de nomes de branch e diretório de worktree.

use crate::WorkItemKind;

/// Remove acentos dos caracteres latinos mais comuns; os demais passam inalterados.
fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// Slug ASCII (`[a-z0-9-]`), sem hífens repetidos nem nas pontas, com no máximo `max_len` bytes.
pub fn slugify(title: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in title.chars().flat_map(char::to_lowercase).map(fold) {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out.truncate(max_len.min(out.len()));
    out.trim_end_matches('-').to_string()
}

/// Branch de trabalho de um work item (spec §7.1).
pub fn branch_for(kind: WorkItemKind, number: u64, title: &str, labels: &[String]) -> String {
    let has = |name: &str| labels.iter().any(|l| l.eq_ignore_ascii_case(name));
    let prefix = match kind {
        WorkItemKind::Epic => "wip",
        WorkItemKind::Issue if has("bug") => "fix",
        WorkItemKind::Issue if has("chore") => "chore",
        WorkItemKind::Issue => "feat",
    };
    let slug = slugify(title, 40);
    if slug.is_empty() {
        format!("{prefix}/{number}")
    } else {
        format!("{prefix}/{number}-{slug}")
    }
}

/// Nome de diretório (um único componente de caminho) para o worktree de uma branch.
pub fn worktree_dir_name(branch: &str) -> String {
    branch.replace('/', "__")
}
```

Observação: `truncate` em `String` só é seguro em limite de caractere; como `out` só contém ASCII, qualquer índice é limite válido.

- [ ] **Step 4: Rodar e ver passar**

Run: `cargo test -p ct-core branch`
Expected: PASS (8 testes).

- [ ] **Step 5: Commit**

```bash
git add crates/ct-core
git commit -m "feat(core): convenções de nome de branch e worktree"
```

---

### Task 4: `ct-core::guard` — guarda-corpo de branch protegida

**Files:**
- Create: `crates/ct-core/src/guard/mod.rs`, `crates/ct-core/src/guard/tokenize.rs`
- Modify: `crates/ct-core/src/lib.rs` (adicionar `pub mod guard;`)
- Test: testes unitários em ambos os arquivos

**Interfaces:**
- Consumes: nada.
- Produces:
  - `ct_core::guard::Verdict { Allow, Block(String) }` (`Debug + PartialEq`)
  - `ct_core::guard::GuardContext<'a> { head_branch: Option<&'a str>, protected: &'a [String], pr_base: &'a dyn Fn(Option<&str>) -> Option<String> }`
  - `ct_core::guard::check_command(cmd: &str, ctx: &GuardContext) -> Verdict`
  - `ct_core::guard::tokenize::split_segments(input: &str) -> Vec<Vec<String>>`

- [ ] **Step 1: Testes do tokenizador (falhando)**

`crates/ct-core/src/guard/tokenize.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn seg(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter().map(|s| s.iter().map(|w| w.to_string()).collect()).collect()
    }

    #[test]
    fn separa_por_operadores() {
        assert_eq!(
            split_segments("git add . && git commit -m x; git push | cat"),
            seg(&[&["git", "add", "."], &["git", "commit", "-m", "x"], &["git", "push"], &["cat"]])
        );
        assert_eq!(split_segments("a || b\nc & d"), seg(&[&["a"], &["b"], &["c"], &["d"]]));
    }

    #[test]
    fn operadores_dentro_de_aspas_nao_separam() {
        assert_eq!(
            split_segments(r#"git commit -m "fix && git push origin develop""#),
            seg(&[&["git", "commit", "-m", "fix && git push origin develop"]])
        );
        assert_eq!(
            split_segments("echo 'a; b' | grep x"),
            seg(&[&["echo", "a; b"], &["grep", "x"]])
        );
    }

    #[test]
    fn escapes_e_aspas_concatenadas() {
        assert_eq!(split_segments(r#"echo a\ b "c\"d" e'f'"#), seg(&[&["echo", "a b", "c\"d", "ef"]]));
    }

    #[test]
    fn string_vazia_entre_aspas_vira_palavra() {
        assert_eq!(split_segments(r#"git commit -m """#), seg(&[&["git", "commit", "-m", ""]]));
    }

    #[test]
    fn entrada_vazia() {
        assert!(split_segments("   ").is_empty());
        assert!(split_segments(";;").is_empty());
    }
}
```

Criar `crates/ct-core/src/guard/mod.rs` com apenas `pub mod tokenize;` e adicionar `pub mod guard;` em `lib.rs`.

Run: `cargo test -p ct-core tokenize`
Expected: FAIL (`cannot find function split_segments`).

- [ ] **Step 2: Implementar o tokenizador** (acima dos testes em `tokenize.rs`)

```rust
//! Tokenização mínima de comandos shell: separa segmentos por `; && || | & \n`
//! e palavras por espaço, respeitando aspas simples, duplas e `\`.
//! Não expande variáveis, globs nem subshells.

pub fn split_segments(input: &str) -> Vec<Vec<String>> {
    let mut segments = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for d in chars.by_ref() {
                    if d == '\'' {
                        break;
                    }
                    cur.push(d);
                }
            }
            '"' => {
                in_word = true;
                while let Some(d) = chars.next() {
                    match d {
                        '"' => break,
                        '\\' => match chars.peek() {
                            Some(&n) if matches!(n, '"' | '\\' | '$' | '`') => {
                                cur.push(n);
                                chars.next();
                            }
                            _ => cur.push('\\'),
                        },
                        _ => cur.push(d),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    if n != '\n' {
                        cur.push(n);
                    }
                }
            }
            ';' | '|' | '&' | '\n' => {
                flush(&mut cur, &mut in_word, &mut words);
                if (c == '&' || c == '|') && chars.peek() == Some(&c) {
                    chars.next();
                }
                if !words.is_empty() {
                    segments.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => flush(&mut cur, &mut in_word, &mut words),
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    flush(&mut cur, &mut in_word, &mut words);
    if !words.is_empty() {
        segments.push(words);
    }
    segments
}

fn flush(cur: &mut String, in_word: &mut bool, words: &mut Vec<String>) {
    if *in_word {
        words.push(std::mem::take(cur));
        *in_word = false;
    }
}
```

Run: `cargo test -p ct-core tokenize`
Expected: PASS (5 testes).

- [ ] **Step 3: Testes do guarda-corpo (falhando)**

Em `crates/ct-core/src/guard/mod.rs`, abaixo de `pub mod tokenize;`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn protected() -> Vec<String> {
        vec!["develop".into(), "main".into(), "master".into()]
    }

    fn check_on(head: Option<&str>, cmd: &str) -> Verdict {
        let p = protected();
        let base = |_: Option<&str>| Some("wip/12-auth".to_string());
        check_command(cmd, &GuardContext { head_branch: head, protected: &p, pr_base: &base })
    }

    fn check_with_pr_base(cmd: &str, pr_base: Option<&str>) -> Verdict {
        let p = protected();
        let b = pr_base.map(str::to_string);
        let base = move |_: Option<&str>| b.clone();
        check_command(cmd, &GuardContext { head_branch: Some("wip/12-auth"), protected: &p, pr_base: &base })
    }

    fn blocked(v: &Verdict) -> bool {
        matches!(v, Verdict::Block(_))
    }

    #[test]
    fn push_para_protegida_bloqueia() {
        for cmd in [
            "git push origin develop",
            "git push origin HEAD:develop",
            "git push origin wip/12:refs/heads/main",
            "git push -f origin +develop",
            "git push origin --delete develop",
            "git push origin :develop",
            "git push --all origin",
            "git push --mirror",
        ] {
            assert!(blocked(&check_on(Some("wip/12-auth"), cmd)), "{cmd}");
        }
    }

    #[test]
    fn push_da_branch_da_sessao_permite() {
        for cmd in [
            "git push",
            "git push -u origin wip/12-auth",
            "git push origin HEAD",
            "git push --set-upstream origin feat/9-x",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn push_implicito_com_head_protegido_bloqueia() {
        assert!(blocked(&check_on(Some("develop"), "git push")));
        assert!(blocked(&check_on(Some("develop"), "git push origin")));
        assert!(blocked(&check_on(Some("develop"), "git push origin HEAD")));
    }

    #[test]
    fn comandos_que_alteram_head_protegido_bloqueiam() {
        for cmd in ["git commit -m x", "git merge wip/12", "git rebase origin/x", "git cherry-pick abc", "git reset --hard", "git revert abc", "git pull"] {
            assert!(blocked(&check_on(Some("develop"), cmd)), "{cmd}");
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn checkout_para_protegida_bloqueia() {
        assert!(blocked(&check_on(Some("wip/12-auth"), "git checkout develop")));
        assert!(blocked(&check_on(Some("wip/12-auth"), "git switch main")));
    }

    #[test]
    fn checkout_que_cria_branch_ou_restaura_arquivo_permite() {
        for cmd in [
            "git switch -c feat/13-x develop",
            "git checkout -b feat/13-x develop",
            "git checkout develop -- Cargo.toml",
            "git checkout wip/12-auth",
        ] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }

    #[test]
    fn opcoes_globais_do_git_sao_ignoradas() {
        assert!(blocked(&check_on(Some("wip/12-auth"), "git -C /repo -c core.x=y --no-pager push origin develop")));
    }

    #[test]
    fn segmentos_e_wrappers_sao_verificados() {
        assert!(blocked(&check_on(Some("wip/12-auth"), "cargo test && git push origin develop")));
        assert!(blocked(&check_on(Some("wip/12-auth"), "GIT_TRACE=1 git push origin develop")));
        assert!(blocked(&check_on(Some("wip/12-auth"), "sudo git push origin develop")));
        assert!(blocked(&check_on(Some("wip/12-auth"), "bash -c 'git push origin develop'")));
        assert!(blocked(&check_on(Some("wip/12-auth"), "/usr/bin/git push origin develop")));
    }

    #[test]
    fn texto_entre_aspas_nao_e_comando() {
        assert_eq!(
            check_on(Some("wip/12-auth"), r#"git commit -m "fix && git push origin develop""#),
            Verdict::Allow
        );
        assert_eq!(check_on(Some("wip/12-auth"), "echo 'git push origin develop'"), Verdict::Allow);
    }

    #[test]
    fn gh_pr_merge_depende_da_base() {
        assert_eq!(check_with_pr_base("gh pr merge 5 --squash", Some("wip/12-auth")), Verdict::Allow);
        assert!(blocked(&check_with_pr_base("gh pr merge 5", Some("develop"))));
        assert!(blocked(&check_with_pr_base("gh pr merge", None)));
        assert!(blocked(&check_with_pr_base("gh -R org/repo pr merge 5", Some("main"))));
    }

    #[test]
    fn gh_api_de_merge_bloqueia() {
        assert!(blocked(&check_on(Some("wip/12-auth"), "gh api -X PUT repos/o/r/pulls/5/merge")));
        assert_eq!(check_on(Some("wip/12-auth"), "gh api repos/o/r/pulls/5"), Verdict::Allow);
    }

    #[test]
    fn head_desconhecido_nao_bloqueia_push_explicito_de_outra_branch() {
        assert_eq!(check_on(None, "git push origin wip/12-auth"), Verdict::Allow);
        assert_eq!(check_on(None, "git commit -m x"), Verdict::Allow);
    }

    #[test]
    fn comandos_sem_relacao_permitem() {
        for cmd in ["cargo test", "ls -la", "git status", "git log --oneline develop", "git diff develop...HEAD", ""] {
            assert_eq!(check_on(Some("wip/12-auth"), cmd), Verdict::Allow, "{cmd}");
        }
    }
}
```

Run: `cargo test -p ct-core guard`
Expected: FAIL de compilação (`cannot find function check_command`).

- [ ] **Step 4: Implementar o guarda-corpo** (entre `pub mod tokenize;` e os testes)

```rust
//! Guarda-corpo de branch protegida (spec §7.3). Puro: o chamador fornece o
//! HEAD atual e a função que descobre a base de um PR.

pub mod tokenize;

use tokenize::split_segments;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block(String),
}

pub struct GuardContext<'a> {
    /// Branch atual do worktree; `None` se desconhecida ou HEAD destacado.
    pub head_branch: Option<&'a str>,
    pub protected: &'a [String],
    /// Base do PR indicado (número/URL/branch), ou do PR da branch atual quando `None`.
    /// Retorna `None` se não conseguir determinar.
    pub pr_base: &'a dyn Fn(Option<&str>) -> Option<String>,
}

impl GuardContext<'_> {
    fn is_protected(&self, branch: &str) -> bool {
        self.protected.iter().any(|p| p == branch)
    }

    fn head_is_protected(&self) -> Option<&str> {
        self.head_branch.filter(|h| self.is_protected(h))
    }
}

const MAX_DEPTH: u8 = 3;

pub fn check_command(cmd: &str, ctx: &GuardContext) -> Verdict {
    check_segments(cmd, ctx, 0)
}

fn check_segments(cmd: &str, ctx: &GuardContext, depth: u8) -> Verdict {
    for seg in split_segments(cmd) {
        let v = check_segment(&seg, ctx, depth);
        if v != Verdict::Allow {
            return v;
        }
    }
    Verdict::Allow
}

fn check_segment(words: &[String], ctx: &GuardContext, depth: u8) -> Verdict {
    let words = skip_env_assignments(words);
    let Some(first) = words.first() else {
        return Verdict::Allow;
    };
    let prog = first.rsplit('/').next().unwrap_or(first);
    match prog {
        "git" => check_git(&words[1..], ctx),
        "gh" => check_gh(&words[1..], ctx),
        "bash" | "sh" | "zsh" | "dash" if depth < MAX_DEPTH => {
            match words.iter().position(|w| w.starts_with('-') && w.ends_with('c')) {
                Some(i) => match words.get(i + 1) {
                    Some(inner) => check_segments(inner, ctx, depth + 1),
                    None => Verdict::Allow,
                },
                None => Verdict::Allow,
            }
        }
        "sudo" | "env" | "command" | "exec" | "nohup" | "time" | "nice" if depth < MAX_DEPTH => {
            let rest: Vec<String> = words[1..].iter().skip_while(|w| w.starts_with('-')).cloned().collect();
            check_segment(&rest, ctx, depth + 1)
        }
        _ => Verdict::Allow,
    }
}

fn skip_env_assignments(words: &[String]) -> &[String] {
    let n = words.iter().take_while(|w| is_assignment(w)).count();
    &words[n..]
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

fn check_git(args: &[String], ctx: &GuardContext) -> Verdict {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    let Some(sub) = args.get(i) else {
        return Verdict::Allow;
    };
    let rest = &args[i + 1..];
    match sub.as_str() {
        "push" => check_push(rest, ctx),
        "merge" | "commit" | "rebase" | "cherry-pick" | "reset" | "revert" | "pull" => match ctx.head_is_protected() {
            Some(h) => Verdict::Block(format!("`git {sub}` com HEAD na branch protegida `{h}`")),
            None => Verdict::Allow,
        },
        "checkout" | "switch" => check_switch(sub, rest, ctx),
        _ => Verdict::Allow,
    }
}

fn check_push(args: &[String], ctx: &GuardContext) -> Verdict {
    let mut positionals: Vec<&str> = Vec::new();
    for a in args {
        match a.as_str() {
            "--all" | "--mirror" | "--branches" => {
                return Verdict::Block(format!("`git push {a}` pode atingir branches protegidas"));
            }
            s if s.starts_with('-') => {}
            s => positionals.push(s),
        }
    }
    let refspecs = positionals.get(1..).unwrap_or(&[]);
    if refspecs.is_empty() {
        return match ctx.head_is_protected() {
            Some(h) => Verdict::Block(format!("`git push` com HEAD na branch protegida `{h}`")),
            None => Verdict::Allow,
        };
    }
    for r in refspecs {
        let r = r.trim_start_matches('+');
        let dst = r.rsplit_once(':').map(|(_, d)| d).unwrap_or(r);
        let dst = if dst == "HEAD" { ctx.head_branch.unwrap_or("") } else { dst };
        let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
        if ctx.is_protected(dst) {
            return Verdict::Block(format!("push para a branch protegida `{dst}`"));
        }
    }
    Verdict::Allow
}

fn check_switch(sub: &str, args: &[String], ctx: &GuardContext) -> Verdict {
    if args.iter().any(|a| a == "--") {
        return Verdict::Allow;
    }
    let creates = args.iter().any(|a| {
        matches!(a.as_str(), "-b" | "-B" | "-c" | "-C" | "--create" | "--force-create" | "--orphan")
    });
    if creates {
        return Verdict::Allow;
    }
    match args.iter().find(|a| !a.starts_with('-') && ctx.is_protected(a)) {
        Some(b) => Verdict::Block(format!("`git {sub}` para a branch protegida `{b}`")),
        None => Verdict::Allow,
    }
}

fn check_gh(args: &[String], ctx: &GuardContext) -> Verdict {
    let pos: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    if let Some(i) = pos.windows(2).position(|w| w == ["pr", "merge"]) {
        let target = pos.get(i + 2).copied();
        return match (ctx.pr_base)(target) {
            Some(base) if !ctx.is_protected(&base) => Verdict::Allow,
            Some(base) => Verdict::Block(format!("`gh pr merge` em PR com base protegida `{base}`")),
            None => Verdict::Block("não foi possível determinar a base do PR para `gh pr merge`".into()),
        };
    }
    if pos.first() == Some(&"api") && pos.iter().any(|a| a.trim_end_matches('/').ends_with("/merge")) {
        return Verdict::Block("merge de PR via `gh api`".into());
    }
    Verdict::Allow
}
```

Observação para `gh -R org/repo pr merge 5`: `org/repo` vira posicional, mas a busca por `["pr","merge"]` em janela continua encontrando o par; o alvo é o elemento seguinte (`5`).

- [ ] **Step 5: Rodar e ver passar**

Run: `cargo test -p ct-core`
Expected: PASS (todos os testes de `model`, `branch`, `tokenize` e `guard`).

- [ ] **Step 6: Commit**

```bash
git add crates/ct-core
git commit -m "feat(core): guarda-corpo de branch protegida"
```

---

### Task 5: `ct-ledger` — esquema, projetos, work items e eventos

**Files:**
- Create: `crates/ct-ledger/Cargo.toml`, `crates/ct-ledger/src/lib.rs`, `src/error.rs`, `src/schema.rs`, `src/projects.rs`, `src/work_items.rs`, `src/events.rs`
- Test: testes unitários em `schema.rs`, `projects.rs`, `work_items.rs`, `events.rs`

**Interfaces:**
- Consumes: `ct_core::{now_ms, WorkItemKind, WorkItemStatus, WorkItemTrigger, next_status, ParseEnumError}`.
- Produces:
  - `ct_ledger::Ledger` com `open(path: &Path) -> Result<Ledger>` e `open_in_memory() -> Result<Ledger>`
  - `ct_ledger::{LedgerError, Result}`
  - `NewProject { name, gh_owner, gh_repo, repo_path, base_branch: String, protected_branches: Vec<String> }`, `Project { id: i64, name, gh_owner, gh_repo, repo_path, base_branch: String, protected_branches: Vec<String>, created_at: i64 }`
  - `Ledger::add_project(&self, &NewProject) -> Result<Project>`, `get_project(i64) -> Result<Project>`, `list_projects() -> Result<Vec<Project>>`
  - `WorkItem { id, project_id, gh_number: i64, kind: WorkItemKind, status: WorkItemStatus, branch: Option<String>, worktree_path: Option<String>, created_at: i64, closed_at: Option<i64> }`
  - `Ledger::ensure_work_item(project_id: i64, gh_number: i64, kind: WorkItemKind) -> Result<WorkItem>`, `get_work_item(i64) -> Result<WorkItem>`, `find_work_item(project_id, gh_number) -> Result<Option<WorkItem>>`, `set_work_item_branch(id, branch: &str, worktree_path: &str) -> Result<()>`, `set_work_item_status(id, WorkItemStatus, cause: &str) -> Result<bool>`, `apply_trigger(id, WorkItemTrigger) -> Result<bool>`
  - `NewEvent<'a> { project_id: i64, work_item_id: Option<i64>, session_id: Option<&'a str>, kind: &'a str, payload: serde_json::Value }`, `Event { id, ts, project_id, work_item_id: Option<i64>, session_id: Option<String>, kind: String, payload: serde_json::Value }`
  - `Ledger::append_event(NewEvent) -> Result<i64>`, `list_events(work_item_id: i64, limit: u32) -> Result<Vec<Event>>` (mais recente primeiro)
  - `#[cfg(test)] crate::test_support::ledger_with_project() -> (Ledger, Project)`

- [ ] **Step 1: Cargo e esqueleto**

`crates/ct-ledger/Cargo.toml`:

```toml
[package]
name = "ct-ledger"
version.workspace = true
edition.workspace = true

[dependencies]
ct-core.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
uuid.workspace = true
```

`crates/ct-ledger/src/error.rs`:

```rust
use ct_core::{ParseEnumError, SessionState};

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0} não encontrado: {1}")]
    NotFound(&'static str, String),
    #[error("transição inválida de sessão: {from} -> {to}")]
    InvalidTransition { from: SessionState, to: SessionState },
    #[error("dado inválido no banco: {0}")]
    Corrupt(String),
}

impl From<ParseEnumError> for LedgerError {
    fn from(e: ParseEnumError) -> Self {
        LedgerError::Corrupt(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LedgerError>;
```

`crates/ct-ledger/src/lib.rs`:

```rust
//! Ledger do Control Tower: persistência SQLite de projetos, work items,
//! sessões, decisões e eventos (spec §5).

mod error;
mod events;
mod projects;
mod schema;
mod work_items;

pub use error::{LedgerError, Result};
pub use events::{Event, NewEvent};
pub use projects::{NewProject, Project};
pub use work_items::WorkItem;

use rusqlite::{types::Type, Connection, Row};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

pub struct Ledger {
    conn: Connection,
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        schema::migrate(&conn)?;
        Ok(Ledger { conn })
    }
}

/// Lê uma coluna TEXT e converte via `FromStr` (enums do domínio).
pub(crate) fn parse_col<T>(row: &Row, idx: usize) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let s: String = row.get(idx)?;
    s.parse::<T>()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

/// Lê uma coluna TEXT contendo JSON.
pub(crate) fn json_col<T: serde::de::DeserializeOwned>(row: &Row, idx: usize) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn ledger_with_project() -> (Ledger, Project) {
        let ledger = Ledger::open_in_memory().unwrap();
        let project = ledger
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/home/u/projetos/demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec!["develop".into(), "main".into()],
            })
            .unwrap();
        (ledger, project)
    }
}
```

- [ ] **Step 2: Esquema — teste (falhando)**

`crates/ct-ledger/src/schema.rs`:

```rust
#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn migracao_e_idempotente() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate(&conn).unwrap();
        super::migrate(&conn).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, super::MIGRATIONS.len() as i64);
    }

    #[test]
    fn event_e_append_only() {
        let conn = Connection::open_in_memory().unwrap();
        super::migrate(&conn).unwrap();
        conn.execute(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, created_at) VALUES ('p','o','r','/x',0)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO event (ts, project_id, type) VALUES (0, 1, 't')", []).unwrap();
        assert!(conn.execute("UPDATE event SET type = 'x'", []).is_err());
        assert!(conn.execute("DELETE FROM event", []).is_err());
    }
}
```

Run: `cargo test -p ct-ledger schema`
Expected: FAIL de compilação (`cannot find function migrate`).

- [ ] **Step 3: Esquema — implementação** (acima dos testes)

```rust
//! Migrações versionadas por `PRAGMA user_version`. Nunca edite uma migração
//! já publicada; acrescente uma nova ao fim de `MIGRATIONS`.

use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE project (
  id                 INTEGER PRIMARY KEY,
  name               TEXT NOT NULL UNIQUE,
  gh_owner           TEXT NOT NULL,
  gh_repo            TEXT NOT NULL,
  repo_path          TEXT NOT NULL,
  base_branch        TEXT NOT NULL DEFAULT 'develop',
  protected_branches TEXT NOT NULL DEFAULT '["develop","main","master"]',
  created_at         INTEGER NOT NULL
);

CREATE TABLE issue_cache (
  project_id     INTEGER NOT NULL REFERENCES project(id),
  gh_number      INTEGER NOT NULL,
  title          TEXT NOT NULL,
  state          TEXT NOT NULL,
  labels         TEXT NOT NULL DEFAULT '[]',
  parent_number  INTEGER,
  has_sub_issues INTEGER NOT NULL DEFAULT 0,
  url            TEXT NOT NULL,
  updated_at     INTEGER NOT NULL,
  synced_at      INTEGER NOT NULL,
  PRIMARY KEY (project_id, gh_number)
);

CREATE TABLE work_item (
  id            INTEGER PRIMARY KEY,
  project_id    INTEGER NOT NULL REFERENCES project(id),
  gh_number     INTEGER NOT NULL,
  kind          TEXT NOT NULL,
  status        TEXT NOT NULL DEFAULT 'backlog',
  branch        TEXT,
  worktree_path TEXT,
  created_at    INTEGER NOT NULL,
  closed_at     INTEGER,
  UNIQUE (project_id, gh_number)
);

CREATE TABLE session (
  id              TEXT PRIMARY KEY,
  work_item_id    INTEGER NOT NULL REFERENCES work_item(id),
  tool            TEXT NOT NULL,
  tool_session_id TEXT,
  transcript_path TEXT,
  backup_path     TEXT,
  cwd             TEXT NOT NULL,
  launch_cmd      TEXT NOT NULL,
  state           TEXT NOT NULL,
  exit_code       INTEGER,
  pid             INTEGER,
  started_at      INTEGER NOT NULL,
  last_event_at   INTEGER NOT NULL,
  ended_at        INTEGER
);
CREATE INDEX session_state_idx ON session(state);

CREATE TABLE decision (
  id             INTEGER PRIMARY KEY,
  work_item_id   INTEGER NOT NULL REFERENCES work_item(id),
  session_id     TEXT REFERENCES session(id),
  origin         TEXT NOT NULL,
  title          TEXT NOT NULL,
  context        TEXT NOT NULL,
  decision       TEXT NOT NULL,
  consequences   TEXT,
  reviewed_at    INTEGER,
  reviewed_in_pr INTEGER,
  created_at     INTEGER NOT NULL
);
CREATE INDEX decision_work_item_idx ON decision(work_item_id);

CREATE TABLE event (
  id           INTEGER PRIMARY KEY,
  ts           INTEGER NOT NULL,
  project_id   INTEGER NOT NULL REFERENCES project(id),
  work_item_id INTEGER REFERENCES work_item(id),
  session_id   TEXT REFERENCES session(id),
  type         TEXT NOT NULL,
  payload      TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX event_work_item_idx ON event(work_item_id, id);

CREATE TRIGGER event_no_update BEFORE UPDATE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
CREATE TRIGGER event_no_delete BEFORE DELETE ON event
BEGIN SELECT RAISE(ABORT, 'event é append-only'); END;
CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
BEGIN SELECT RAISE(ABORT, 'decision não pode ser apagada'); END;
CREATE TRIGGER decision_immutable
BEFORE UPDATE OF work_item_id, session_id, origin, title, context, decision, consequences, created_at ON decision
BEGIN SELECT RAISE(ABORT, 'decision só pode ter a revisão alterada'); END;
"#];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}
```

Para compilar o esqueleto antes dos próximos steps, crie `projects.rs`, `work_items.rs` e `events.rs` com o conteúdo dos Steps 4–6 (os testes falharão até a implementação estar completa).

Run: `cargo test -p ct-ledger schema`
Expected: PASS (2 testes) — após os Steps 4–6 compilarem.

- [ ] **Step 4: Eventos — testes e implementação**

`crates/ct-ledger/src/events.rs`:

```rust
use crate::{json_col, Ledger, Result};
use ct_core::now_ms;
use rusqlite::{params, Row};
use serde::Serialize;

pub struct NewEvent<'a> {
    pub project_id: i64,
    pub work_item_id: Option<i64>,
    pub session_id: Option<&'a str>,
    pub kind: &'a str,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    pub id: i64,
    pub ts: i64,
    pub project_id: i64,
    pub work_item_id: Option<i64>,
    pub session_id: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
}

fn row_to_event(r: &Row) -> rusqlite::Result<Event> {
    Ok(Event {
        id: r.get(0)?,
        ts: r.get(1)?,
        project_id: r.get(2)?,
        work_item_id: r.get(3)?,
        session_id: r.get(4)?,
        kind: r.get(5)?,
        payload: json_col(r, 6)?,
    })
}

impl Ledger {
    pub fn append_event(&self, e: NewEvent<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO event (ts, project_id, work_item_id, session_id, type, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now_ms(), e.project_id, e.work_item_id, e.session_id, e.kind, e.payload.to_string()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Eventos do work item, do mais recente para o mais antigo.
    pub fn list_events(&self, work_item_id: i64, limit: u32) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, project_id, work_item_id, session_id, type, payload
             FROM event WHERE work_item_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![work_item_id, limit], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use ct_core::WorkItemKind;
    use serde_json::json;

    #[test]
    fn append_e_lista_do_mais_recente() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        l.append_event(NewEvent { project_id: p.id, work_item_id: Some(wi.id), session_id: None, kind: "a", payload: json!({"n": 1}) }).unwrap();
        l.append_event(NewEvent { project_id: p.id, work_item_id: Some(wi.id), session_id: None, kind: "b", payload: json!({}) }).unwrap();
        let evs = l.list_events(wi.id, 10).unwrap();
        assert_eq!(evs[0].kind, "b");
        assert_eq!(evs[1].kind, "a");
        assert_eq!(evs[1].payload, json!({"n": 1}));
        // +1 do evento work_item_created
        assert_eq!(evs.len(), 3);
    }
}
```

- [ ] **Step 5: Projetos — testes e implementação**

`crates/ct-ledger/src/projects.rs`:

```rust
use crate::{json_col, Ledger, LedgerError, Result};
use ct_core::now_ms;
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

pub struct NewProject {
    pub name: String,
    pub gh_owner: String,
    pub gh_repo: String,
    pub repo_path: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub gh_owner: String,
    pub gh_repo: String,
    pub repo_path: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
    pub created_at: i64,
}

const COLS: &str = "id, name, gh_owner, gh_repo, repo_path, base_branch, protected_branches, created_at";

fn row_to_project(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        gh_owner: r.get(2)?,
        gh_repo: r.get(3)?,
        repo_path: r.get(4)?,
        base_branch: r.get(5)?,
        protected_branches: json_col(r, 6)?,
        created_at: r.get(7)?,
    })
}

impl Ledger {
    pub fn add_project(&self, p: &NewProject) -> Result<Project> {
        let protected = serde_json::to_string(&p.protected_branches).expect("Vec<String> serializa");
        self.conn.execute(
            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, base_branch, protected_branches, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![p.name, p.gh_owner, p.gh_repo, p.repo_path, p.base_branch, protected, now_ms()],
        )?;
        self.get_project(self.conn.last_insert_rowid())
    }

    pub fn get_project(&self, id: i64) -> Result<Project> {
        self.conn
            .query_row(&format!("SELECT {COLS} FROM project WHERE id = ?1"), [id], row_to_project)
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("project", id.to_string()))
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {COLS} FROM project ORDER BY name"))?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ledger_with_project;
    use crate::LedgerError;

    #[test]
    fn adiciona_e_le_projeto() {
        let (l, p) = ledger_with_project();
        assert_eq!(p.protected_branches, vec!["develop", "main"]);
        assert_eq!(l.get_project(p.id).unwrap(), p);
        assert_eq!(l.list_projects().unwrap(), vec![p]);
    }

    #[test]
    fn projeto_inexistente() {
        let (l, _) = ledger_with_project();
        assert!(matches!(l.get_project(999), Err(LedgerError::NotFound("project", _))));
    }
}
```

- [ ] **Step 6: Work items — testes e implementação**

`crates/ct-ledger/src/work_items.rs`:

```rust
use crate::{parse_col, Ledger, LedgerError, NewEvent, Result};
use ct_core::{next_status, now_ms, WorkItemKind, WorkItemStatus, WorkItemTrigger};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkItem {
    pub id: i64,
    pub project_id: i64,
    pub gh_number: i64,
    pub kind: WorkItemKind,
    pub status: WorkItemStatus,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

const COLS: &str = "id, project_id, gh_number, kind, status, branch, worktree_path, created_at, closed_at";

fn row_to_work_item(r: &Row) -> rusqlite::Result<WorkItem> {
    Ok(WorkItem {
        id: r.get(0)?,
        project_id: r.get(1)?,
        gh_number: r.get(2)?,
        kind: parse_col(r, 3)?,
        status: parse_col(r, 4)?,
        branch: r.get(5)?,
        worktree_path: r.get(6)?,
        created_at: r.get(7)?,
        closed_at: r.get(8)?,
    })
}

impl Ledger {
    /// Cria o work item se não existir; sempre retorna o registro atual.
    pub fn ensure_work_item(&self, project_id: i64, gh_number: i64, kind: WorkItemKind) -> Result<WorkItem> {
        let tx = self.conn.unchecked_transaction()?;
        let inserted = self.conn.execute(
            "INSERT INTO work_item (project_id, gh_number, kind, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (project_id, gh_number) DO NOTHING",
            params![project_id, gh_number, kind.as_str(), now_ms()],
        )?;
        let wi = self
            .find_work_item(project_id, gh_number)?
            .ok_or_else(|| LedgerError::NotFound("work_item", format!("{project_id}#{gh_number}")))?;
        if inserted == 1 {
            self.append_event(NewEvent {
                project_id,
                work_item_id: Some(wi.id),
                session_id: None,
                kind: "work_item_created",
                payload: json!({ "gh_number": gh_number, "kind": kind }),
            })?;
        }
        tx.commit()?;
        Ok(wi)
    }

    pub fn get_work_item(&self, id: i64) -> Result<WorkItem> {
        self.conn
            .query_row(&format!("SELECT {COLS} FROM work_item WHERE id = ?1"), [id], row_to_work_item)
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("work_item", id.to_string()))
    }

    pub fn find_work_item(&self, project_id: i64, gh_number: i64) -> Result<Option<WorkItem>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM work_item WHERE project_id = ?1 AND gh_number = ?2"),
                params![project_id, gh_number],
                row_to_work_item,
            )
            .optional()?)
    }

    pub fn set_work_item_branch(&self, id: i64, branch: &str, worktree_path: &str) -> Result<()> {
        let wi = self.get_work_item(id)?;
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE work_item SET branch = ?1, worktree_path = ?2 WHERE id = ?3",
            params![branch, worktree_path, id],
        )?;
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(id),
            session_id: None,
            kind: "worktree_set",
            payload: json!({ "branch": branch, "worktree_path": worktree_path }),
        })?;
        tx.commit()?;
        Ok(())
    }

    /// Muda o status e registra evento. Retorna `false` se já estava no status.
    pub fn set_work_item_status(&self, id: i64, status: WorkItemStatus, cause: &str) -> Result<bool> {
        let wi = self.get_work_item(id)?;
        if wi.status == status {
            return Ok(false);
        }
        let closed_at = (status == WorkItemStatus::Concluido).then(now_ms);
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE work_item SET status = ?1, closed_at = ?2 WHERE id = ?3",
            params![status.as_str(), closed_at, id],
        )?;
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(id),
            session_id: None,
            kind: "status_changed",
            payload: json!({ "from": wi.status, "to": status, "cause": cause }),
        })?;
        tx.commit()?;
        Ok(true)
    }

    pub fn apply_trigger(&self, id: i64, trigger: WorkItemTrigger) -> Result<bool> {
        let wi = self.get_work_item(id)?;
        match next_status(wi.status, trigger) {
            Some(s) => self.set_work_item_status(id, s, &format!("{trigger:?}")),
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ledger_with_project;
    use ct_core::{WorkItemKind, WorkItemStatus, WorkItemTrigger};

    #[test]
    fn ensure_e_idempotente() {
        let (l, p) = ledger_with_project();
        let a = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let b = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.status, WorkItemStatus::Backlog);
        let created = l.list_events(a.id, 10).unwrap().iter().filter(|e| e.kind == "work_item_created").count();
        assert_eq!(created, 1);
    }

    #[test]
    fn status_muda_e_registra_evento() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        assert!(l.set_work_item_status(wi.id, WorkItemStatus::Pausado, "manual").unwrap());
        assert!(!l.set_work_item_status(wi.id, WorkItemStatus::Pausado, "manual").unwrap());
        let ev = &l.list_events(wi.id, 1).unwrap()[0];
        assert_eq!(ev.kind, "status_changed");
        assert_eq!(ev.payload["to"], "pausado");
    }

    #[test]
    fn concluido_registra_fechamento() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        l.apply_trigger(wi.id, WorkItemTrigger::PrMerged).unwrap();
        let wi = l.get_work_item(wi.id).unwrap();
        assert_eq!(wi.status, WorkItemStatus::Concluido);
        assert!(wi.closed_at.is_some());
        assert!(!l.apply_trigger(wi.id, WorkItemTrigger::PrOpened).unwrap());
    }

    #[test]
    fn branch_e_worktree() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        l.set_work_item_branch(wi.id, "wip/12-auth", "/w/demo/wip__12-auth").unwrap();
        let wi = l.get_work_item(wi.id).unwrap();
        assert_eq!(wi.branch.as_deref(), Some("wip/12-auth"));
        assert_eq!(wi.worktree_path.as_deref(), Some("/w/demo/wip__12-auth"));
    }
}
```

- [ ] **Step 7: Rodar e ver passar**

Run: `cargo test -p ct-ledger`
Expected: PASS (schema 2, events 1, projects 2, work_items 4).

- [ ] **Step 8: Commit**

```bash
git add crates/ct-ledger
git commit -m "feat(ledger): esquema SQLite, projetos, work items e eventos"
```

---

### Task 6: `ct-ledger` — sessões, recuperação e decisões

**Files:**
- Create: `crates/ct-ledger/src/sessions.rs`, `crates/ct-ledger/src/decisions.rs`
- Modify: `crates/ct-ledger/src/lib.rs` (módulos e re-exports)
- Test: testes unitários nos dois arquivos

**Interfaces:**
- Consumes: Task 5 (`Ledger`, `NewEvent`, `parse_col`, `json_col`, `get_work_item`), `ct_core::{SessionState, Tool, DecisionOrigin, now_ms}`.
- Produces:
  - `NewSession { work_item_id: i64, tool: Tool, cwd: String, launch_cmd: Vec<String> }`
  - `Session { id: String, work_item_id: i64, tool: Tool, tool_session_id: Option<String>, transcript_path: Option<String>, backup_path: Option<String>, cwd: String, launch_cmd: Vec<String>, state: SessionState, exit_code: Option<i32>, pid: Option<i64>, started_at: i64, last_event_at: i64, ended_at: Option<i64> }`
  - `Ledger::create_session(&NewSession) -> Result<Session>`, `get_session(&str) -> Result<Session>`, `set_session_pid(&str, i64) -> Result<()>`, `set_session_tool_info(&str, tool_session_id: &str, transcript_path: &str) -> Result<()>`, `set_session_backup_path(&str, &str) -> Result<()>`, `transition_session(&str, SessionState, exit_code: Option<i32>) -> Result<bool>`, `live_sessions() -> Result<Vec<Session>>`, `sessions_in_state(SessionState) -> Result<Vec<Session>>`, `mark_interrupted(is_alive: &dyn Fn(i64) -> bool) -> Result<Vec<String>>`
  - `NewDecision { work_item_id: i64, session_id: Option<String>, origin: DecisionOrigin, title: String, context: String, decision: String, consequences: Option<String> }`
  - `Decision { id: i64, work_item_id: i64, session_id: Option<String>, origin: DecisionOrigin, title, context, decision: String, consequences: Option<String>, reviewed_at: Option<i64>, reviewed_in_pr: Option<i64>, created_at: i64 }`
  - `Ledger::record_decision(&NewDecision) -> Result<Decision>`, `get_decision(i64) -> Result<Decision>`, `list_decisions(work_item_ids: &[i64], only_unreviewed: bool) -> Result<Vec<Decision>>`, `mark_reviewed(ids: &[i64], pr_number: i64) -> Result<usize>`

Semântica de `transition_session`:
- mesmo estado → só atualiza `last_event_at`, retorna `Ok(false)`;
- transição inválida → `Err(InvalidTransition)`;
- para `exited`: `ended_at = agora`; para `interrupted`: `ended_at = last_event_at anterior` e `last_event_at` **não** muda (preserva "quando parou").

- [ ] **Step 1: Registrar módulos**

Em `lib.rs`, adicionar `mod decisions; mod sessions;` e:

```rust
pub use decisions::{Decision, NewDecision};
pub use sessions::{NewSession, Session};
```

- [ ] **Step 2: Sessões — testes (falhando)**

`crates/ct-ledger/src/sessions.rs` (testes primeiro):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use crate::LedgerError;
    use ct_core::{SessionState, Tool, WorkItemKind};

    fn new_session(l: &Ledger, project_id: i64) -> Session {
        let wi = l.ensure_work_item(project_id, 12, WorkItemKind::Epic).unwrap();
        l.create_session(&NewSession {
            work_item_id: wi.id,
            tool: Tool::Claude,
            cwd: "/w/demo/wip__12-auth".into(),
            launch_cmd: vec!["claude".into(), "prompt".into()],
        })
        .unwrap()
    }

    #[test]
    fn cria_sessao_em_starting() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        assert_eq!(s.state, SessionState::Starting);
        assert_eq!(s.launch_cmd, vec!["claude", "prompt"]);
        assert_eq!(l.get_session(&s.id).unwrap(), s);
    }

    #[test]
    fn transicoes_e_info_da_ferramenta() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        l.set_session_pid(&s.id, 4242).unwrap();
        l.set_session_tool_info(&s.id, "abc-123", "/home/u/.claude/projects/x/abc-123.jsonl").unwrap();
        assert!(l.transition_session(&s.id, SessionState::Running, None).unwrap());
        assert!(!l.transition_session(&s.id, SessionState::Running, None).unwrap());
        assert!(l.transition_session(&s.id, SessionState::Exited, Some(1)).unwrap());
        let s = l.get_session(&s.id).unwrap();
        assert_eq!(s.pid, Some(4242));
        assert_eq!(s.tool_session_id.as_deref(), Some("abc-123"));
        assert_eq!(s.exit_code, Some(1));
        assert!(s.ended_at.is_some());
    }

    #[test]
    fn transicao_invalida_e_erro() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        l.transition_session(&s.id, SessionState::Exited, Some(0)).unwrap();
        let err = l.transition_session(&s.id, SessionState::Running, None).unwrap_err();
        assert!(matches!(err, LedgerError::InvalidTransition { .. }));
    }

    #[test]
    fn mark_interrupted_so_afeta_vivas_sem_processo() {
        let (l, p) = ledger_with_project();
        let viva = new_session(&l, p.id);
        l.set_session_pid(&viva.id, 1).unwrap();
        l.transition_session(&viva.id, SessionState::Running, None).unwrap();
        let morta = new_session(&l, p.id);
        l.set_session_pid(&morta.id, 2).unwrap();
        l.transition_session(&morta.id, SessionState::Idle, None).unwrap();
        let sem_pid = new_session(&l, p.id);
        let encerrada = new_session(&l, p.id);
        l.transition_session(&encerrada.id, SessionState::Exited, Some(0)).unwrap();

        let antes = l.get_session(&morta.id).unwrap().last_event_at;
        let mut ids = l.mark_interrupted(&|pid| pid == 1).unwrap();
        ids.sort();
        let mut esperado = vec![morta.id.clone(), sem_pid.id.clone()];
        esperado.sort();
        assert_eq!(ids, esperado);

        let m = l.get_session(&morta.id).unwrap();
        assert_eq!(m.state, SessionState::Interrupted);
        assert_eq!(m.last_event_at, antes);
        assert_eq!(m.ended_at, Some(antes));
        assert_eq!(l.get_session(&viva.id).unwrap().state, SessionState::Running);
        assert_eq!(l.get_session(&encerrada.id).unwrap().state, SessionState::Exited);
    }

    #[test]
    fn mark_interrupted_repetido_nao_duplica() {
        let (l, p) = ledger_with_project();
        let s = new_session(&l, p.id);
        assert_eq!(l.mark_interrupted(&|_| false).unwrap().len(), 1);
        assert!(l.mark_interrupted(&|_| false).unwrap().is_empty());
        let wi = l.get_session(&s.id).unwrap().work_item_id;
        let n = l
            .list_events(wi, 100)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "session_state" && e.payload["to"] == "interrupted")
            .count();
        assert_eq!(n, 1);
        assert_eq!(l.sessions_in_state(SessionState::Interrupted).unwrap().len(), 1);
    }
}
```

Run: `cargo test -p ct-ledger sessions`
Expected: FAIL de compilação (`cannot find struct NewSession`).

- [ ] **Step 3: Sessões — implementação** (acima dos testes)

```rust
use crate::{json_col, parse_col, Ledger, LedgerError, NewEvent, Result};
use ct_core::{now_ms, SessionState, Tool};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;

pub struct NewSession {
    pub work_item_id: i64,
    pub tool: Tool,
    pub cwd: String,
    pub launch_cmd: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Session {
    pub id: String,
    pub work_item_id: i64,
    pub tool: Tool,
    pub tool_session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub backup_path: Option<String>,
    pub cwd: String,
    pub launch_cmd: Vec<String>,
    pub state: SessionState,
    pub exit_code: Option<i32>,
    pub pid: Option<i64>,
    pub started_at: i64,
    pub last_event_at: i64,
    pub ended_at: Option<i64>,
}

const COLS: &str = "id, work_item_id, tool, tool_session_id, transcript_path, backup_path, cwd, \
                    launch_cmd, state, exit_code, pid, started_at, last_event_at, ended_at";

fn row_to_session(r: &Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        work_item_id: r.get(1)?,
        tool: parse_col(r, 2)?,
        tool_session_id: r.get(3)?,
        transcript_path: r.get(4)?,
        backup_path: r.get(5)?,
        cwd: r.get(6)?,
        launch_cmd: json_col(r, 7)?,
        state: parse_col(r, 8)?,
        exit_code: r.get(9)?,
        pid: r.get(10)?,
        started_at: r.get(11)?,
        last_event_at: r.get(12)?,
        ended_at: r.get(13)?,
    })
}

impl Ledger {
    pub fn create_session(&self, s: &NewSession) -> Result<Session> {
        let wi = self.get_work_item(s.work_item_id)?;
        let id = uuid::Uuid::new_v4().to_string();
        let launch_cmd = serde_json::to_string(&s.launch_cmd).expect("Vec<String> serializa");
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO session (id, work_item_id, tool, cwd, launch_cmd, state, started_at, last_event_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'starting', ?6, ?6)",
            params![id, s.work_item_id, s.tool.as_str(), s.cwd, launch_cmd, now_ms()],
        )?;
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: Some(&id),
            kind: "session_created",
            payload: json!({ "tool": s.tool, "cwd": s.cwd }),
        })?;
        tx.commit()?;
        self.get_session(&id)
    }

    pub fn get_session(&self, id: &str) -> Result<Session> {
        self.conn
            .query_row(&format!("SELECT {COLS} FROM session WHERE id = ?1"), [id], row_to_session)
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("session", id.to_string()))
    }

    fn update_session(&self, id: &str, sql: &str, value: &dyn rusqlite::ToSql) -> Result<()> {
        let n = self.conn.execute(sql, params![value, id])?;
        if n == 0 {
            return Err(LedgerError::NotFound("session", id.to_string()));
        }
        Ok(())
    }

    pub fn set_session_pid(&self, id: &str, pid: i64) -> Result<()> {
        self.update_session(id, "UPDATE session SET pid = ?1 WHERE id = ?2", &pid)
    }

    pub fn set_session_backup_path(&self, id: &str, path: &str) -> Result<()> {
        self.update_session(id, "UPDATE session SET backup_path = ?1 WHERE id = ?2", &path)
    }

    pub fn set_session_tool_info(&self, id: &str, tool_session_id: &str, transcript_path: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE session SET tool_session_id = ?1, transcript_path = ?2 WHERE id = ?3",
            params![tool_session_id, transcript_path, id],
        )?;
        if n == 0 {
            return Err(LedgerError::NotFound("session", id.to_string()));
        }
        Ok(())
    }

    pub fn transition_session(&self, id: &str, to: SessionState, exit_code: Option<i32>) -> Result<bool> {
        let s = self.get_session(id)?;
        let now = now_ms();
        if s.state == to {
            self.conn
                .execute("UPDATE session SET last_event_at = ?1 WHERE id = ?2", params![now, id])?;
            return Ok(false);
        }
        if !s.state.can_transition_to(to) {
            return Err(LedgerError::InvalidTransition { from: s.state, to });
        }
        let (last_event_at, ended_at) = match to {
            SessionState::Interrupted => (s.last_event_at, Some(s.last_event_at)),
            SessionState::Exited => (now, Some(now)),
            _ => (now, s.ended_at),
        };
        let wi = self.get_work_item(s.work_item_id)?;
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE session SET state = ?1, exit_code = COALESCE(?2, exit_code),
                    last_event_at = ?3, ended_at = ?4
             WHERE id = ?5",
            params![to.as_str(), exit_code, last_event_at, ended_at, id],
        )?;
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: Some(id),
            kind: "session_state",
            payload: json!({ "from": s.state, "to": to, "exit_code": exit_code }),
        })?;
        tx.commit()?;
        Ok(true)
    }

    pub fn live_sessions(&self) -> Result<Vec<Session>> {
        self.query_sessions("state IN ('starting', 'running', 'idle')", &[])
    }

    pub fn sessions_in_state(&self, state: SessionState) -> Result<Vec<Session>> {
        self.query_sessions("state = ?1", &[&state.as_str()])
    }

    fn query_sessions(&self, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Session>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLS} FROM session WHERE {filter} ORDER BY started_at"))?;
        let rows = stmt.query_map(args, row_to_session)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Marca como `interrupted` toda sessão viva cujo processo não existe mais
    /// (ou que nunca registrou PID). Retorna os ids afetados. Usado no boot do daemon.
    pub fn mark_interrupted(&self, is_alive: &dyn Fn(i64) -> bool) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for s in self.live_sessions()? {
            if !s.pid.is_some_and(is_alive) {
                self.transition_session(&s.id, SessionState::Interrupted, None)?;
                ids.push(s.id);
            }
        }
        Ok(ids)
    }
}
```

Run: `cargo test -p ct-ledger sessions`
Expected: PASS (5 testes).

- [ ] **Step 4: Decisões — testes (falhando)**

`crates/ct-ledger/src/decisions.rs` (testes primeiro):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use ct_core::{DecisionOrigin, WorkItemKind};

    fn nd(work_item_id: i64, origin: DecisionOrigin, title: &str) -> NewDecision {
        NewDecision {
            work_item_id,
            session_id: None,
            origin,
            title: title.into(),
            context: "ctx".into(),
            decision: "dec".into(),
            consequences: None,
        }
    }

    #[test]
    fn autonoma_nasce_pendente_e_orientada_revisada() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l.record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a")).unwrap();
        let o = l.record_decision(&nd(wi.id, DecisionOrigin::Orientada, "o")).unwrap();
        assert!(a.reviewed_at.is_none());
        assert_eq!(o.reviewed_at, Some(o.created_at));
        let pend = l.list_decisions(&[wi.id], true).unwrap();
        assert_eq!(pend.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(), vec!["a"]);
        assert_eq!(l.list_decisions(&[wi.id], false).unwrap().len(), 2);
    }

    #[test]
    fn lista_varios_work_items() {
        let (l, p) = ledger_with_project();
        let w1 = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let w2 = l.ensure_work_item(p.id, 13, WorkItemKind::Issue).unwrap();
        l.record_decision(&nd(w1.id, DecisionOrigin::Autonoma, "1")).unwrap();
        l.record_decision(&nd(w2.id, DecisionOrigin::Autonoma, "2")).unwrap();
        assert_eq!(l.list_decisions(&[w1.id, w2.id], true).unwrap().len(), 2);
        assert!(l.list_decisions(&[], true).unwrap().is_empty());
    }

    #[test]
    fn marca_revisadas_uma_vez() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l.record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a")).unwrap();
        let o = l.record_decision(&nd(wi.id, DecisionOrigin::Orientada, "o")).unwrap();
        assert_eq!(l.mark_reviewed(&[a.id, o.id], 31).unwrap(), 1);
        assert_eq!(l.mark_reviewed(&[a.id], 32).unwrap(), 0);
        assert_eq!(l.get_decision(a.id).unwrap().reviewed_in_pr, Some(31));
        assert!(l.mark_reviewed(&[], 1).unwrap() == 0);
    }

    #[test]
    fn conteudo_da_decisao_e_imutavel() {
        let (l, p) = ledger_with_project();
        let wi = l.ensure_work_item(p.id, 12, WorkItemKind::Epic).unwrap();
        let a = l.record_decision(&nd(wi.id, DecisionOrigin::Autonoma, "a")).unwrap();
        assert!(l.conn.execute("UPDATE decision SET title = 'x' WHERE id = ?1", [a.id]).is_err());
        assert!(l.conn.execute("DELETE FROM decision WHERE id = ?1", [a.id]).is_err());
    }
}
```

Run: `cargo test -p ct-ledger decisions`
Expected: FAIL de compilação.

- [ ] **Step 5: Decisões — implementação** (acima dos testes)

```rust
use crate::{parse_col, Ledger, LedgerError, NewEvent, Result};
use ct_core::{now_ms, DecisionOrigin};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, OptionalExtension, Row};
use serde::Serialize;
use serde_json::json;

pub struct NewDecision {
    pub work_item_id: i64,
    pub session_id: Option<String>,
    pub origin: DecisionOrigin,
    pub title: String,
    pub context: String,
    pub decision: String,
    pub consequences: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    pub id: i64,
    pub work_item_id: i64,
    pub session_id: Option<String>,
    pub origin: DecisionOrigin,
    pub title: String,
    pub context: String,
    pub decision: String,
    pub consequences: Option<String>,
    pub reviewed_at: Option<i64>,
    pub reviewed_in_pr: Option<i64>,
    pub created_at: i64,
}

const COLS: &str = "id, work_item_id, session_id, origin, title, context, decision, consequences, \
                    reviewed_at, reviewed_in_pr, created_at";

fn row_to_decision(r: &Row) -> rusqlite::Result<Decision> {
    Ok(Decision {
        id: r.get(0)?,
        work_item_id: r.get(1)?,
        session_id: r.get(2)?,
        origin: parse_col(r, 3)?,
        title: r.get(4)?,
        context: r.get(5)?,
        decision: r.get(6)?,
        consequences: r.get(7)?,
        reviewed_at: r.get(8)?,
        reviewed_in_pr: r.get(9)?,
        created_at: r.get(10)?,
    })
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

impl Ledger {
    /// Registra decisão. `orientada`/`manual` nascem revisadas; `autonoma` fica pendente.
    pub fn record_decision(&self, d: &NewDecision) -> Result<Decision> {
        let wi = self.get_work_item(d.work_item_id)?;
        let now = now_ms();
        let reviewed_at = (d.origin != DecisionOrigin::Autonoma).then_some(now);
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "INSERT INTO decision (work_item_id, session_id, origin, title, context, decision,
                                   consequences, reviewed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                d.work_item_id,
                d.session_id,
                d.origin.as_str(),
                d.title,
                d.context,
                d.decision,
                d.consequences,
                reviewed_at,
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.append_event(NewEvent {
            project_id: wi.project_id,
            work_item_id: Some(wi.id),
            session_id: d.session_id.as_deref(),
            kind: "decision_recorded",
            payload: json!({ "decision_id": id, "origin": d.origin, "title": d.title }),
        })?;
        tx.commit()?;
        self.get_decision(id)
    }

    pub fn get_decision(&self, id: i64) -> Result<Decision> {
        self.conn
            .query_row(&format!("SELECT {COLS} FROM decision WHERE id = ?1"), [id], row_to_decision)
            .optional()?
            .ok_or_else(|| LedgerError::NotFound("decision", id.to_string()))
    }

    pub fn list_decisions(&self, work_item_ids: &[i64], only_unreviewed: bool) -> Result<Vec<Decision>> {
        if work_item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut sql = format!(
            "SELECT {COLS} FROM decision WHERE work_item_id IN ({})",
            placeholders(work_item_ids.len())
        );
        if only_unreviewed {
            sql.push_str(" AND reviewed_at IS NULL");
        }
        sql.push_str(" ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(work_item_ids.iter()), row_to_decision)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Marca como revisadas as decisões ainda pendentes entre `ids`. Retorna quantas mudaram.
    pub fn mark_reviewed(&self, ids: &[i64], pr_number: i64) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "UPDATE decision SET reviewed_at = ?, reviewed_in_pr = ?
             WHERE reviewed_at IS NULL AND id IN ({})",
            placeholders(ids.len())
        );
        let mut values = vec![Value::Integer(now_ms()), Value::Integer(pr_number)];
        values.extend(ids.iter().map(|i| Value::Integer(*i)));
        Ok(self.conn.execute(&sql, params_from_iter(values))?)
    }
}
```

- [ ] **Step 6: Rodar tudo**

Run: `cargo test -p ct-ledger && cargo clippy -p ct-ledger --all-targets -- -D warnings`
Expected: PASS (todos os testes do crate) e clippy sem avisos.

- [ ] **Step 7: Commit**

```bash
git add crates/ct-ledger
git commit -m "feat(ledger): sessões, recuperação após queda e decisões"
```

---

### Task 7: `ct-git` — worktrees

**Files:**
- Create: `crates/ct-git/Cargo.toml`, `crates/ct-git/src/lib.rs`
- Test: `crates/ct-git/tests/worktree.rs`

**Interfaces:**
- Consumes: nada (usa o binário `git` do sistema).
- Produces:
  - `ct_git::WorktreeSpec<'a> { repo_path: &'a Path, branch: &'a str, base: &'a str, worktree_path: &'a Path }`
  - `ct_git::WorktreeOutcome { Created, Reused }` (`Debug + PartialEq`)
  - `ct_git::GitError { Failed { args: String, stderr: String }, BranchMismatch { path: PathBuf, expected: String, actual: String }, Io(std::io::Error) }`
  - `ct_git::ensure_worktree(&WorktreeSpec) -> Result<WorktreeOutcome, GitError>`
  - `ct_git::current_branch(path: &Path) -> Result<Option<String>, GitError>` (`None` = HEAD destacado)
  - `ct_git::remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(), GitError>` (sem `--force`; falha se houver mudanças)

Regras de `ensure_worktree`: caminho existe → `Reused` se a branch bate, senão `BranchMismatch` (sem alterar nada). Caminho não existe → usa a branch local se existir; senão a remota `origin/<branch>` (com tracking); senão cria a partir de `origin/<base>` se existir, senão de `<base>`.

- [ ] **Step 1: Cargo**

`crates/ct-git/Cargo.toml`:

```toml
[package]
name = "ct-git"
version.workspace = true
edition.workspace = true

[dependencies]
thiserror.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

`crates/ct-git/src/lib.rs` provisório: `//! Operações git do Control Tower.`

- [ ] **Step 2: Testes (falhando)**

`crates/ct-git/tests/worktree.rs`:

```rust
use ct_git::{current_branch, ensure_worktree, remove_worktree, GitError, WorktreeOutcome, WorktreeSpec};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo() -> TempDir {
    let d = TempDir::new().unwrap();
    git(d.path(), &["init", "-q", "-b", "develop"]);
    git(d.path(), &["config", "user.email", "t@t"]);
    git(d.path(), &["config", "user.name", "t"]);
    git(d.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    d
}

#[test]
fn cria_worktree_com_branch_nova_a_partir_da_base() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("demo/wip__12-auth");
    let spec = WorktreeSpec { repo_path: r.path(), branch: "wip/12-auth", base: "develop", worktree_path: &wt };
    assert_eq!(ensure_worktree(&spec).unwrap(), WorktreeOutcome::Created);
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("wip/12-auth"));
    git(r.path(), &["merge-base", "--is-ancestor", "develop", "wip/12-auth"]);
}

#[test]
fn segunda_chamada_reutiliza() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec { repo_path: r.path(), branch: "wip/12-auth", base: "develop", worktree_path: &wt };
    ensure_worktree(&spec).unwrap();
    assert_eq!(ensure_worktree(&spec).unwrap(), WorktreeOutcome::Reused);
}

#[test]
fn usa_branch_local_existente() {
    let r = repo();
    git(r.path(), &["branch", "feat/1-x"]);
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec { repo_path: r.path(), branch: "feat/1-x", base: "develop", worktree_path: &wt };
    assert_eq!(ensure_worktree(&spec).unwrap(), WorktreeOutcome::Created);
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("feat/1-x"));
}

#[test]
fn usa_branch_remota_com_tracking() {
    let origin = repo();
    git(origin.path(), &["branch", "wip/9-fisica"]);
    let clones = TempDir::new().unwrap();
    let clone = clones.path().join("clone");
    let out = Command::new("git")
        .args(["clone", "-q"])
        .arg(origin.path())
        .arg(&clone)
        .output()
        .unwrap();
    assert!(out.status.success());
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec { repo_path: &clone, branch: "wip/9-fisica", base: "develop", worktree_path: &wt };
    ensure_worktree(&spec).unwrap();
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("wip/9-fisica"));
    assert_eq!(git(&wt, &["rev-parse", "--abbrev-ref", "@{upstream}"]), "origin/wip/9-fisica");
}

#[test]
fn caminho_ocupado_por_outra_branch_falha_sem_alterar() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let a = WorktreeSpec { repo_path: r.path(), branch: "wip/1-a", base: "develop", worktree_path: &wt };
    ensure_worktree(&a).unwrap();
    let b = WorktreeSpec { repo_path: r.path(), branch: "wip/2-b", base: "develop", worktree_path: &wt };
    let err = ensure_worktree(&b).unwrap_err();
    assert!(matches!(err, GitError::BranchMismatch { ref actual, .. } if actual == "wip/1-a"));
    assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("wip/1-a"));
    assert!(git(r.path(), &["branch", "--list", "wip/2-b"]).is_empty());
}

#[test]
fn base_inexistente_gera_erro_do_git() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec { repo_path: r.path(), branch: "wip/3-c", base: "nao-existe", worktree_path: &wt };
    assert!(matches!(ensure_worktree(&spec), Err(GitError::Failed { .. })));
}

#[test]
fn remove_worktree_limpo() {
    let r = repo();
    let wts = TempDir::new().unwrap();
    let wt = wts.path().join("wt");
    let spec = WorktreeSpec { repo_path: r.path(), branch: "wip/4-d", base: "develop", worktree_path: &wt };
    ensure_worktree(&spec).unwrap();
    remove_worktree(r.path(), &wt).unwrap();
    assert!(!wt.exists());
}
```

Run: `cargo test -p ct-git`
Expected: FAIL de compilação (`unresolved imports`).

- [ ] **Step 3: Implementar** (`crates/ct-git/src/lib.rs`)

```rust
//! Operações git do Control Tower (via `git` CLI).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git {args} falhou: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("worktree em {path:?} está na branch `{actual}`, esperado `{expected}`")]
    BranchMismatch { path: PathBuf, expected: String, actual: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct WorktreeSpec<'a> {
    pub repo_path: &'a Path,
    pub branch: &'a str,
    pub base: &'a str,
    pub worktree_path: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeOutcome {
    Created,
    Reused,
}

fn run(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        return Err(GitError::Failed {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn ref_exists(dir: &Path, reference: &str) -> Result<bool, GitError> {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["show-ref", "--verify", "--quiet", reference])
        .status()?;
    Ok(status.success())
}

/// Branch atual do diretório; `None` se HEAD estiver destacado.
pub fn current_branch(path: &Path) -> Result<Option<String>, GitError> {
    let b = run(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok((b != "HEAD").then_some(b))
}

pub fn ensure_worktree(spec: &WorktreeSpec) -> Result<WorktreeOutcome, GitError> {
    if spec.worktree_path.exists() {
        let actual = current_branch(spec.worktree_path)?.unwrap_or_else(|| "HEAD".into());
        if actual == spec.branch {
            return Ok(WorktreeOutcome::Reused);
        }
        return Err(GitError::BranchMismatch {
            path: spec.worktree_path.to_path_buf(),
            expected: spec.branch.to_string(),
            actual,
        });
    }
    if let Some(parent) = spec.worktree_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let repo = spec.repo_path;
    let wt = spec.worktree_path.to_string_lossy().into_owned();
    let remote_branch = format!("origin/{}", spec.branch);

    if ref_exists(repo, &format!("refs/heads/{}", spec.branch))? {
        run(repo, &["worktree", "add", wt.as_str(), spec.branch])?;
    } else if ref_exists(repo, &format!("refs/remotes/{remote_branch}"))? {
        run(repo, &["worktree", "add", "--track", "-b", spec.branch, wt.as_str(), remote_branch.as_str()])?;
    } else {
        let remote_base = format!("origin/{}", spec.base);
        let base = if ref_exists(repo, &format!("refs/remotes/{remote_base}"))? {
            remote_base
        } else {
            spec.base.to_string()
        };
        run(repo, &["worktree", "add", "--no-track", "-b", spec.branch, wt.as_str(), base.as_str()])?;
    }
    Ok(WorktreeOutcome::Created)
}

/// Remove o worktree. Falha (sem `--force`) se houver mudanças não commitadas.
pub fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<(), GitError> {
    let wt = worktree_path.to_string_lossy().into_owned();
    run(repo_path, &["worktree", "remove", wt.as_str()])?;
    Ok(())
}
```

- [ ] **Step 4: Rodar e ver passar**

Run: `cargo test -p ct-git`
Expected: PASS (7 testes).

- [ ] **Step 5: Commit**

```bash
git add crates/ct-git
git commit -m "feat(git): criação e reutilização de worktrees por work item"
```

---

### Task 8: CLI `ctower hook` — entrega, spool e guarda-corpo

**Files:**
- Create: `crates/ctower/Cargo.toml`, `crates/ctower/src/main.rs`, `crates/ctower/src/hook.rs`
- Test: `crates/ctower/tests/hook_cli.rs`

**Interfaces:**
- Consumes: `ct_core::guard::{check_command, GuardContext, Verdict}`, `ct_core::now_ms` (Tasks 2 e 4).
- Produces:
  - binário `ctower` com subcomando `ctower hook <session-start|user-prompt-submit|notification|stop|session-end|pre-tool-use>`, lendo o JSON do hook no stdin.
  - Contrato HTTP consumido pelo daemon (Plano 2): `POST $CTOWER_URL/hooks`, `Content-Type: application/json`, `Authorization: Bearer $CTOWER_TOKEN` (se definido), corpo `{"session": "<CTOWER_SESSION>", "event": "<SessionStart|UserPromptSubmit|Notification|Stop|SessionEnd|GuardBlocked>", "payload": <json do stdin ou null>, "ts": <ms>}`.
  - Spool: arquivos `<ms>-<pid>.json` (mesmo corpo) em `$CTOWER_SPOOL_DIR` ou `~/.local/share/ctower/hooks-spool/`; escrita atômica (temporário oculto + rename).
  - Variáveis lidas: `CTOWER_SESSION`, `CTOWER_URL`, `CTOWER_TOKEN`, `CTOWER_PROTECTED`, `CTOWER_SPOOL_DIR`, `CTOWER_GH` (binário do gh; padrão `gh`, sobrescrito em testes).
  - Códigos de saída: sempre `0`, exceto `pre-tool-use` bloqueado → `2` com motivo em stderr.

- [ ] **Step 1: Cargo e `main.rs`**

`crates/ctower/Cargo.toml`:

```toml
[package]
name = "ctower"
version.workspace = true
edition.workspace = true

[dependencies]
ct-core.workspace = true
clap.workspace = true
serde_json.workspace = true
ureq.workspace = true

[dev-dependencies]
assert_cmd.workspace = true
predicates.workspace = true
tempfile.workspace = true
```

`crates/ctower/src/main.rs`:

```rust
//! CLI auxiliar do Control Tower: ponto de entrada dos hooks e diagnóstico.

mod hook;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ctower", version, about = "CLI auxiliar do Control Tower")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ponto de entrada dos hooks do Claude Code (lê o JSON do hook no stdin).
    Hook {
        #[arg(value_enum)]
        event: hook::HookEvent,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Hook { event } => hook::run(event),
    };
    std::process::exit(code);
}
```

- [ ] **Step 2: Testes de integração (falhando)**

`crates/ctower/tests/hook_cli.rs`:

```rust
use assert_cmd::Command;
use predicates::str::contains;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command as StdCommand;
use std::time::Duration;
use tempfile::TempDir;

fn ctower() -> Command {
    let mut c = Command::cargo_bin("ctower").unwrap();
    for v in ["CTOWER_SESSION", "CTOWER_URL", "CTOWER_TOKEN", "CTOWER_PROTECTED", "CTOWER_SPOOL_DIR", "CTOWER_GH"] {
        c.env_remove(v);
    }
    c
}

fn git(dir: &Path, args: &[&str]) {
    let st = StdCommand::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(st.status.success(), "git {args:?}");
}

fn repo_on(branch: &str) -> TempDir {
    let d = TempDir::new().unwrap();
    git(d.path(), &["init", "-q", "-b", branch]);
    git(d.path(), &["config", "user.email", "t@t"]);
    git(d.path(), &["config", "user.name", "t"]);
    git(d.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
    d
}

fn bash_payload(command: &str, cwd: &Path) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
    })
    .to_string()
}

fn spool_files(dir: &Path) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json") && !n.starts_with('.'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn sem_sessao_nunca_bloqueia() {
    let r = repo_on("wip/12-auth");
    ctower()
        .args(["hook", "pre-tool-use"])
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .success();
}

#[test]
fn bloqueia_push_para_develop() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .code(2)
        .stderr(contains("protegida"));
    assert_eq!(spool_files(spool.path()).len(), 1, "evento GuardBlocked vai para o spool");
}

#[test]
fn permite_push_da_branch_da_sessao() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("git push -u origin wip/12-auth", r.path()))
        .assert()
        .success();
    assert!(spool_files(spool.path()).is_empty());
}

#[test]
fn bloqueia_commit_com_head_em_develop() {
    let r = repo_on("develop");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(bash_payload("git commit -m x", r.path()))
        .assert()
        .code(2);
}

#[test]
fn protegidas_vem_do_ambiente() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_PROTECTED", "release, trunk")
        .write_stdin(bash_payload("git push origin develop", r.path()))
        .assert()
        .success();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_PROTECTED", "release, trunk")
        .write_stdin(bash_payload("git push origin trunk", r.path()))
        .assert()
        .code(2);
}

#[test]
fn gh_pr_merge_sem_base_conhecida_bloqueia() {
    let r = repo_on("wip/12-auth");
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .env("CTOWER_GH", "false")
        .write_stdin(bash_payload("gh pr merge 5 --squash", r.path()))
        .assert()
        .code(2);
}

#[test]
fn ferramenta_que_nao_e_bash_passa() {
    let spool = TempDir::new().unwrap();
    let payload = serde_json::json!({"tool_name": "Edit", "tool_input": {"file_path": "x"}}).to_string();
    ctower()
        .args(["hook", "pre-tool-use"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(payload)
        .assert()
        .success();
}

#[test]
fn stdin_invalido_nao_bloqueia() {
    let spool = TempDir::new().unwrap();
    for event in ["pre-tool-use", "stop", "session-start"] {
        ctower()
            .args(["hook", event])
            .env("CTOWER_SESSION", "s1")
            .env("CTOWER_SPOOL_DIR", spool.path())
            .write_stdin("isto não é json")
            .assert()
            .success();
    }
}

#[test]
fn sem_daemon_vai_para_o_spool() {
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "stop"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_URL", "http://127.0.0.1:9")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(r#"{"session_id":"abc"}"#)
        .timeout(Duration::from_secs(10))
        .assert()
        .success();
    let files = spool_files(spool.path());
    assert_eq!(files.len(), 1);
    let body = std::fs::read_to_string(spool.path().join(&files[0])).unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["event"], "Stop");
    assert_eq!(v["session"], "s1");
    assert_eq!(v["payload"]["session_id"], "abc");
}

fn serve_once(listener: TcpListener) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).into_owned();
            if let Some(h) = text.find("\r\n\r\n") {
                let len = text[..h]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if buf.len() >= h + 4 + len {
                    break;
                }
            }
        }
        s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    })
}

#[test]
fn entrega_ao_daemon_com_token() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = serve_once(listener);
    let spool = TempDir::new().unwrap();
    ctower()
        .args(["hook", "notification"])
        .env("CTOWER_SESSION", "s1")
        .env("CTOWER_URL", &url)
        .env("CTOWER_TOKEN", "tok")
        .env("CTOWER_SPOOL_DIR", spool.path())
        .write_stdin(r#"{"message":"precisa de permissão"}"#)
        .assert()
        .success();
    let req = server.join().unwrap();
    assert!(req.starts_with("POST /hooks"), "{req}");
    assert!(req.contains("Bearer tok"), "{req}");
    assert!(req.contains(r#""event":"Notification""#), "{req}");
    assert!(spool_files(spool.path()).is_empty());
}
```

Run: `cargo test -p ctower`
Expected: FAIL de compilação (`file not found for module hook`).

- [ ] **Step 3: Implementar `hook.rs`**

`crates/ctower/src/hook.rs`:

```rust
//! `ctower hook <evento>`: repassa eventos do Claude Code ao daemon e aplica o
//! guarda-corpo de branch protegida. Nunca bloqueia o agente por falha de
//! comunicação (spec §7.2, §7.3).

use clap::ValueEnum;
use ct_core::guard::{check_command, GuardContext, Verdict};
use ct_core::now_ms;
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    Notification,
    Stop,
    SessionEnd,
    PreToolUse,
}

impl HookEvent {
    fn wire_name(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Notification => "Notification",
            HookEvent::Stop => "Stop",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::PreToolUse => "PreToolUse",
        }
    }
}

const DEFAULT_PROTECTED: &str = "develop,main,master";

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn run(event: HookEvent) -> i32 {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Some(session) = env_nonempty("CTOWER_SESSION") else {
        return 0;
    };
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    match event {
        HookEvent::PreToolUse => pre_tool_use(&session, &payload),
        other => {
            deliver(&session, other.wire_name(), &payload);
            0
        }
    }
}

fn pre_tool_use(session: &str, payload: &Value) -> i32 {
    if payload.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return 0;
    }
    let Some(cmd) = payload.pointer("/tool_input/command").and_then(Value::as_str) else {
        return 0;
    };
    let cwd: Option<PathBuf> = payload
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let protected: Vec<String> = env_nonempty("CTOWER_PROTECTED")
        .unwrap_or_else(|| DEFAULT_PROTECTED.to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let head = cwd.as_deref().and_then(git_head);
    let gh = env_nonempty("CTOWER_GH").unwrap_or_else(|| "gh".to_string());
    let pr_base = |target: Option<&str>| gh_pr_base(&gh, cwd.as_deref(), target);
    let ctx = GuardContext { head_branch: head.as_deref(), protected: &protected, pr_base: &pr_base };

    match check_command(cmd, &ctx) {
        Verdict::Allow => 0,
        Verdict::Block(reason) => {
            eprintln!(
                "[Control Tower] Bloqueado: {reason} (branch protegida). \
                 Trabalhe apenas na branch da sessão; o merge na base é feito pelo humano."
            );
            deliver(session, "GuardBlocked", &json!({ "command": cmd, "reason": reason }));
            2
        }
    }
}

fn git_head(cwd: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty() && b != "HEAD").then_some(b)
}

fn gh_pr_base(gh: &str, cwd: Option<&Path>, target: Option<&str>) -> Option<String> {
    let mut c = Command::new(gh);
    c.args(["pr", "view"]);
    if let Some(t) = target {
        c.arg(t);
    }
    c.args(["--json", "baseRefName", "--jq", ".baseRefName"]);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn deliver(session: &str, event: &str, payload: &Value) {
    let body = json!({ "session": session, "event": event, "payload": payload, "ts": now_ms() }).to_string();
    let sent = env_nonempty("CTOWER_URL").is_some_and(|url| post(&url, &body));
    if !sent {
        let _ = spool(&body);
    }
}

fn post(base: &str, body: &str) -> bool {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(1)).build();
    let mut req = agent
        .post(&format!("{}/hooks", base.trim_end_matches('/')))
        .set("Content-Type", "application/json");
    if let Some(t) = env_nonempty("CTOWER_TOKEN") {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    req.send_string(body).is_ok()
}

fn spool_dir() -> PathBuf {
    if let Some(d) = env_nonempty("CTOWER_SPOOL_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share/ctower/hooks-spool")
}

fn spool(body: &str) -> std::io::Result<()> {
    let dir = spool_dir();
    fs::create_dir_all(&dir)?;
    let name = format!("{}-{}.json", now_ms(), std::process::id());
    let tmp = dir.join(format!(".{name}.tmp"));
    fs::write(&tmp, body)?;
    fs::rename(tmp, dir.join(name))
}
```

- [ ] **Step 4: Rodar e ver passar**

Run: `cargo test -p ctower`
Expected: PASS (10 testes).

- [ ] **Step 5: Verificação completa do workspace**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: tudo verde.

- [ ] **Step 6: Teste manual com o Claude Code real** (verifica a superfície de hooks — risco §16 da spec)

Criar `~/tmp-hook-test/.claude/settings.json` num repositório de teste em branch `develop`:

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [{ "type": "command", "command": "~/projetos/control-tower/target/debug/ctower hook pre-tool-use" }] }
    ]
  }
}
```

Run: `cd ~/tmp-hook-test && CTOWER_SESSION=manual claude -p "rode: git commit --allow-empty -m teste"`
Expected: o Claude relata que o comando foi bloqueado com a mensagem "[Control Tower] Bloqueado: ..."; `git log` não mostra o commit. Registrar no PR qualquer divergência do formato de payload (`tool_name`, `tool_input.command`, `cwd`) para ajuste no Plano 2.

- [ ] **Step 7: Commit e PR**

```bash
git add crates/ctower
git commit -m "feat(ctower): CLI de hooks com guarda-corpo e spool offline"
git push -u origin feat/plano-1-fundacao
gh pr create --base develop --title "feat: Plano 1 — fundação (core, ledger, git, hooks)" \
  --body "Implementa docs/superpowers/plans/2026-10-15-plano-1-fundacao.md"
```
