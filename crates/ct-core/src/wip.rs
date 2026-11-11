//! Limite de trabalho em andamento por projeto (issue #126, épico #178): o
//! máximo de sessões de implementação vivas ao mesmo tempo e o de PRs
//! abertos por work items do projeto. Vazio = sem limite. O hub conta o uso
//! (sessões vivas de todos os ambientes e PRs abertos do espelho); aqui ficam
//! só os tipos e a regra pura.

use serde::{Deserialize, Serialize};

/// Teto aceito em cada limite: acima disso é engano de digitação, não limite.
pub const WIP_LIMIT_MAX: u32 = 1000;

/// Texto que a UI, o gestor e as recusas mostram com o limite atingido.
pub const WIP_LIMIT_REACHED: &str = "limite de trabalho em andamento";

/// Limites do projeto (`HubProject.wip_limits`). Ausente num hub anterior:
/// sem limite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WipLimits {
    /// Máximo de sessões de implementação vivas (em issues e épicos, em
    /// todos os ambientes); `None` = sem limite.
    pub max_sessions: Option<u32>,
    /// Máximo de PRs abertos por work items do projeto; `None` = sem limite.
    pub max_prs: Option<u32>,
}

impl WipLimits {
    /// JSON da coluna `wip_limits`.
    pub fn config_json(&self) -> String {
        serde_json::to_string(self).expect("WipLimits serializa")
    }

    /// Nenhum limite configurado.
    pub fn is_empty(&self) -> bool {
        self.max_sessions.is_none() && self.max_prs.is_none()
    }

    /// Cada limite fica entre 1 e [`WIP_LIMIT_MAX`] (zero bloquearia tudo:
    /// para isso, desligue o modo autônomo).
    pub fn validate(&self) -> Result<(), String> {
        for (label, v) in [
            ("sessões simultâneas", self.max_sessions),
            ("PRs abertos", self.max_prs),
        ] {
            if let Some(n) = v {
                if n == 0 || n > WIP_LIMIT_MAX {
                    return Err(format!(
                        "limite de {label}: use de 1 a {WIP_LIMIT_MAX}, ou vazio para sem limite"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Uso e limites do projeto agora (`Hub::project_wip`). É o que o agendador
/// do piloto automático (#183) consulta antes de iniciar uma sessão.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WipUsage {
    /// Sessões de implementação vivas, de todos os ambientes.
    pub sessions: u32,
    /// PRs abertos do espelho ligados a work items do projeto.
    pub prs: u32,
    pub limits: WipLimits,
}

impl WipUsage {
    /// Sessões no limite (ou acima).
    pub fn sessions_full(&self) -> bool {
        self.limits.max_sessions.is_some_and(|m| self.sessions >= m)
    }

    /// PRs no limite (ou acima).
    pub fn prs_full(&self) -> bool {
        self.limits.max_prs.is_some_and(|m| self.prs >= m)
    }

    /// Cabe mais uma sessão de implementação: nenhum dos limites atingido.
    pub fn has_room(&self) -> bool {
        !self.sessions_full() && !self.prs_full()
    }

    /// Motivo legível, com o "N de M" de cada limite atingido; `None` = há
    /// vaga.
    pub fn reason(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let (true, Some(m)) = (self.sessions_full(), self.limits.max_sessions) {
            parts.push(format!("{} de {m} sessões", self.sessions));
        }
        if let (true, Some(m)) = (self.prs_full(), self.limits.max_prs) {
            parts.push(format!("{} de {m} PRs abertos", self.prs));
        }
        (!parts.is_empty()).then(|| format!("{WIP_LIMIT_REACHED} ({})", parts.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(sessions: u32, prs: u32, ms: Option<u32>, mp: Option<u32>) -> WipUsage {
        WipUsage {
            sessions,
            prs,
            limits: WipLimits {
                max_sessions: ms,
                max_prs: mp,
            },
        }
    }

    #[test]
    fn sem_limite_sempre_tem_vaga() {
        let u = usage(50, 50, None, None);
        assert!(u.has_room());
        assert_eq!(u.reason(), None);
        assert!(u.limits.is_empty());
    }

    #[test]
    fn limite_atingido_e_motivo_com_n_de_m() {
        assert!(usage(1, 0, Some(2), None).has_room());
        let u = usage(2, 3, Some(2), Some(5));
        assert!(!u.has_room() && u.sessions_full() && !u.prs_full());
        assert_eq!(
            u.reason().unwrap(),
            "limite de trabalho em andamento (2 de 2 sessões)"
        );
        let u = usage(3, 5, Some(2), Some(5));
        assert_eq!(
            u.reason().unwrap(),
            "limite de trabalho em andamento (3 de 2 sessões, 5 de 5 PRs abertos)"
        );
    }

    #[test]
    fn valida_os_limites_e_aceita_json_parcial() {
        assert!(WipLimits::default().validate().is_ok());
        let zero = WipLimits {
            max_sessions: Some(0),
            max_prs: None,
        };
        assert!(zero.validate().unwrap_err().contains("sessões simultâneas"));
        let big = WipLimits {
            max_sessions: None,
            max_prs: Some(WIP_LIMIT_MAX + 1),
        };
        assert!(big.validate().unwrap_err().contains("PRs abertos"));
        let l: WipLimits = serde_json::from_str(r#"{"max_prs":3}"#).unwrap();
        assert_eq!(l.max_prs, Some(3));
        assert_eq!(l.max_sessions, None);
        let l: WipLimits = serde_json::from_str("{}").unwrap();
        assert!(l.is_empty());
    }
}
