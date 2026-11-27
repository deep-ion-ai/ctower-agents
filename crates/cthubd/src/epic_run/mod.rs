//! Execução de épico (épico #295): o plano por sub-issue, aprovado pelo
//! desenvolvedor, executado por um motor determinístico no hub. O estado
//! fica no `ct-hub-store` (`epic_run`); a decisão, pura, em [`plan`]. A
//! sessão de plano (#299) grava pela rota de agente
//! `POST /agent/v1/epic/plan`, com o prompt de [`prompt`].

pub mod plan;
pub mod prompt;
pub mod rpc;
pub mod runner;

pub use runner::{spawn, Engine};
