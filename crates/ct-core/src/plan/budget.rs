//! Política de orçamento do plano por conta (issue #180, épico #178): diz
//! se uma sessão **autônoma** nova pode começar agora.
//!
//! A cota é da conta, então a política limita **até onde** as sessões
//! autônomas podem levar o uso da conta, não quanto elas gastam. A
//! avaliação ([`evaluate`]) é pura: recebe a política, a visão do plano da
//! conta ([`PlanView`], já com a regra do reset) e o relógio, e devolve
//! [`BudgetDecision`]. O agendador (#183) chama [`evaluate`] antes de
//! iniciar cada issue; sessões em andamento nunca são interrompidas por ela.
//!
//! # Regras
//!
//! - **Faixas** ([`TimeBand`]): dias da semana, início e fim (minutos do
//!   dia no fuso da política; fim menor que o início atravessa a
//!   meia-noite e pertence ao dia do início). Cada faixa é fechada
//!   (`five_hour_cap = None`) ou tem um teto da janela de 5h. Faixas
//!   sobrepostas: vale a primeira da lista. Fora de qualquer faixa conta
//!   como fechado.
//! - **Dias úteis:** os dias que têm uma faixa marcada `workday` (no padrão,
//!   o horário comercial).
//! - **Corte de início:** num dia útil, do corte até o início da primeira
//!   faixa de dia útil que começa depois dele, nenhuma sessão nova inicia
//!   (no padrão, das 04:00 às 08:00). Dia útil sem faixa de dia útil depois
//!   do corte não tem janela de corte.
//! - **Limite da semanal:** `min(T, 100 − R × d, ritmo)`, com `d` os dias
//!   úteis cuja faixa de dia útil ainda não terminou até o reset. O ritmo é
//!   a trajetória esperada da semana: a reserva dos dias úteis já passados
//!   (`R × (W − d)`, `W` = dias úteis da semana) mais a parte autônoma
//!   (`max(0, T − R × W)`) na proporção `p` das horas elegíveis da semana
//!   já passadas. Assim a parte autônoma se distribui pela semana em vez de
//!   ir toda na primeira noite. `p` conta a ocorrência da faixa elegível
//!   atual até o fim dela (a cota da faixa fica disponível desde o início).
//! - **Leitura:** vale a mais recente, sem sonda. Janela renovada
//!   (`agora >= resets_at`) conta como 0%; a semanal renovada estima o
//!   próximo reset somando semanas ao último conhecido. Janela sem nenhuma
//!   leitura bloqueia ([`BlockReason::NoReading`]).
//!
//! O fuso é um deslocamento fixo do UTC (sem horário de verão).

use super::{PlanView, WindowView};
use serde::{Deserialize, Serialize};

const MIN_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
const WEEK_MS: i64 = 7 * DAY_MS;
const DAY_MIN: u16 = 1440;

/// Uma faixa de horário da política.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TimeBand {
    /// Nome para a UI ("Horário comercial").
    pub label: String,
    /// Dias da semana em que a faixa começa: 0 = segunda … 6 = domingo.
    pub days: Vec<u8>,
    /// Início, em minutos do dia (0..1440) no fuso da política.
    pub start_min: u16,
    /// Fim, em minutos do dia (1..=1440; 1440 = 24:00). Menor que o início:
    /// atravessa a meia-noite e termina no dia seguinte.
    pub end_min: u16,
    /// Teto da janela de 5h (0 a 100) nesta faixa; `None` = faixa fechada.
    #[serde(default)]
    pub five_hour_cap: Option<f64>,
    /// Faixa de dia útil: conta para a reserva R e para o corte de início.
    #[serde(default)]
    pub workday: bool,
}

/// Política de orçamento de uma conta (`hub.autopilot.policy.get|set`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BudgetPolicy {
    /// Fuso: minutos em relação ao UTC (-180 = GMT-3).
    pub utc_offset_min: i32,
    pub bands: Vec<TimeBand>,
    /// Teto semanal T (0 a 100).
    pub weekly_cap: f64,
    /// Reserva R por dia útil (0 a 100).
    pub reserve_per_workday: f64,
    /// Corte de início nos dias úteis, em minutos do dia; `None` = sem corte.
    #[serde(default)]
    pub workday_cutoff_min: Option<u16>,
    /// Horário do resumo da manhã (#185), em minutos do dia no fuso da
    /// política; sai em todo dia útil ([`BudgetPolicy::is_workday`]).
    /// Ausente na política salva = o padrão, 09:00; `None` (nulo) = sem
    /// resumo.
    #[serde(default = "default_digest_at")]
    pub digest_at_min: Option<u16>,
}

/// Horário padrão do resumo da manhã: 09:00.
pub const DIGEST_AT_DEFAULT_MIN: u16 = 9 * 60;

fn default_digest_at() -> Option<u16> {
    Some(DIGEST_AT_DEFAULT_MIN)
}

impl Default for BudgetPolicy {
    /// Padrão da issue #180: dias úteis das 08:00 às 18:00 fechados, noite
    /// de dia útil com 5h até 60%, fim de semana com 5h até 95%, T = 80%,
    /// R = 12%, corte às 04:00, fuso GMT-3.
    fn default() -> Self {
        let band = |label: &str, days: &[u8], start, end, cap, workday| TimeBand {
            label: label.into(),
            days: days.to_vec(),
            start_min: start,
            end_min: end,
            five_hour_cap: cap,
            workday,
        };
        const WEEKDAYS: &[u8] = &[0, 1, 2, 3, 4];
        BudgetPolicy {
            utc_offset_min: -180,
            bands: vec![
                band("Horário comercial", WEEKDAYS, 8 * 60, 18 * 60, None, true),
                band(
                    "Madrugada de dia útil",
                    WEEKDAYS,
                    0,
                    8 * 60,
                    Some(60.0),
                    false,
                ),
                band(
                    "Noite de dia útil",
                    WEEKDAYS,
                    18 * 60,
                    DAY_MIN,
                    Some(60.0),
                    false,
                ),
                band("Fim de semana", &[5, 6], 0, DAY_MIN, Some(95.0), false),
            ],
            weekly_cap: 80.0,
            reserve_per_workday: 12.0,
            workday_cutoff_min: Some(4 * 60),
            digest_at_min: default_digest_at(),
        }
    }
}

impl BudgetPolicy {
    /// Confere os campos; o erro diz o que corrigir (vai para a UI).
    pub fn validate(&self) -> Result<(), String> {
        let pct = |v: f64, what: &str| {
            if v.is_finite() && (0.0..=100.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("{what} deve ficar entre 0 e 100"))
            }
        };
        if !(-720..=840).contains(&self.utc_offset_min) {
            return Err("fuso fora de UTC-12:00 a UTC+14:00".into());
        }
        pct(self.weekly_cap, "teto semanal")?;
        pct(self.reserve_per_workday, "reserva por dia útil")?;
        if self.workday_cutoff_min.is_some_and(|m| m >= DAY_MIN) {
            return Err("corte de início fora do dia (00:00 a 23:59)".into());
        }
        if self.digest_at_min.is_some_and(|m| m >= DAY_MIN) {
            return Err("horário do resumo fora do dia (00:00 a 23:59)".into());
        }
        if self.bands.is_empty() {
            return Err("a política precisa de ao menos uma faixa".into());
        }
        for b in &self.bands {
            let name = if b.label.trim().is_empty() {
                "faixa sem nome"
            } else {
                b.label.as_str()
            };
            if b.days.is_empty() || b.days.iter().any(|d| *d > 6) {
                return Err(format!(
                    "{name}: dias da semana de 0 (segunda) a 6 (domingo)"
                ));
            }
            if b.start_min >= DAY_MIN || b.end_min == 0 || b.end_min > DAY_MIN {
                return Err(format!("{name}: horário fora do dia"));
            }
            if b.start_min == b.end_min {
                return Err(format!("{name}: início e fim iguais"));
            }
            if let Some(c) = b.five_hour_cap {
                pct(c, &format!("{name}: teto da 5h"))?;
            }
        }
        Ok(())
    }

    /// Dia local (dias desde 1970-01-01 no fuso da política) de `t`.
    pub fn local_day(&self, t: i64) -> i64 {
        (t + i64::from(self.utc_offset_min) * MIN_MS).div_euclid(DAY_MS)
    }

    /// Início (UTC, ms) do dia local `day`.
    fn day_start(&self, day: i64) -> i64 {
        day * DAY_MS - i64::from(self.utc_offset_min) * MIN_MS
    }

    /// Dia útil: o dia local tem alguma faixa marcada `workday` **e** é de
    /// segunda a sexta. O resumo da manhã (#185) nunca sai em fim de
    /// semana, mesmo com uma faixa de dia útil no sábado ou no domingo.
    pub fn is_workday(&self, day: i64) -> bool {
        let wd = weekday(day);
        wd < 5 && self.bands.iter().any(|b| b.workday && b.days.contains(&wd))
    }

    /// O resumo da manhã está devido em `now`? Devolve o dia local dele
    /// quando: há horário, hoje (no fuso da política) é dia útil, já passou
    /// do horário e o último resumo enviado (`last_day`, dia local) é de
    /// antes de hoje. Uma vez por dia: depois de enviar hoje, `None` até o
    /// próximo dia útil. Hub fora do ar no horário: sai quando voltar, no
    /// mesmo dia.
    pub fn digest_due(&self, now: i64, last_day: Option<i64>) -> Option<i64> {
        let at = self.digest_at_min?;
        let day = self.local_day(now);
        if !self.is_workday(day) || last_day.is_some_and(|d| d >= day) {
            return None;
        }
        (now >= self.day_start(day) + i64::from(at) * MIN_MS).then_some(day)
    }

    /// Intervalos (UTC) de cada faixa que começa no dia local `day`.
    fn occurrences(&self, day: i64) -> impl Iterator<Item = (usize, i64, i64)> + '_ {
        let base = self.day_start(day);
        let wd = weekday(day);
        self.bands
            .iter()
            .enumerate()
            .filter(move |(_, b)| b.days.contains(&wd))
            .map(move |(i, b)| {
                let end = if b.end_min > b.start_min {
                    b.end_min
                } else {
                    b.end_min + DAY_MIN
                };
                (
                    i,
                    base + i64::from(b.start_min) * MIN_MS,
                    base + i64::from(end) * MIN_MS,
                )
            })
    }

    /// Faixa que vale em `t` (a primeira da lista que o contém) e o fim
    /// dessa ocorrência dela.
    fn band_at(&self, t: i64) -> Option<(usize, i64)> {
        let day = self.local_day(t);
        (day - 1..=day)
            .flat_map(|d| self.occurrences(d))
            .filter(|(_, s, e)| *s <= t && t < *e)
            .map(|(i, _, e)| (i, e))
            .min_by_key(|(i, _)| *i)
    }

    /// Janela de corte do dia local `day` (UTC): do corte até o início da
    /// primeira faixa de dia útil que começa nele depois do corte.
    fn cutoff_window(&self, day: i64) -> Option<(i64, i64)> {
        let cut = self.workday_cutoff_min?;
        let wd = weekday(day);
        let first = self
            .bands
            .iter()
            .filter(|b| b.workday && b.days.contains(&wd) && b.start_min > cut)
            .map(|b| b.start_min)
            .min()?;
        let base = self.day_start(day);
        Some((
            base + i64::from(cut) * MIN_MS,
            base + i64::from(first) * MIN_MS,
        ))
    }

    fn in_cutoff(&self, t: i64) -> bool {
        self.cutoff_window(self.local_day(t))
            .is_some_and(|(s, e)| s <= t && t < e)
    }

    /// Pelo horário (sem olhar o uso): faixa aberta e fora do corte.
    fn eligible(&self, t: i64) -> bool {
        self.band_at(t)
            .is_some_and(|(i, _)| self.bands[i].five_hour_cap.is_some())
            && !self.in_cutoff(t)
    }

    /// Instantes em `[a, b]` onde a elegibilidade pode mudar, ordenados.
    fn breakpoints(&self, a: i64, b: i64) -> Vec<i64> {
        let mut out = vec![a, b];
        for day in self.local_day(a) - 1..=self.local_day(b) + 1 {
            for (_, s, e) in self.occurrences(day) {
                out.extend([s, e]);
            }
            if let Some((s, e)) = self.cutoff_window(day) {
                out.extend([s, e]);
            }
        }
        out.retain(|t| (a..=b).contains(t));
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Segmentos `[s, e)` de elegibilidade constante em `[a, b)`.
    fn segments(&self, a: i64, b: i64) -> Vec<(i64, i64, bool)> {
        self.breakpoints(a, b)
            .windows(2)
            .map(|w| (w[0], w[1], self.eligible(w[0])))
            .collect()
    }

    /// Milissegundos elegíveis em `[a, b)`.
    fn eligible_ms(&self, a: i64, b: i64) -> i64 {
        if b <= a {
            return 0;
        }
        self.segments(a, b)
            .into_iter()
            .filter(|(_, _, ok)| *ok)
            .map(|(s, e, _)| e - s)
            .sum()
    }

    /// Próximo instante `>= now` em que o horário permite iniciar; `None`
    /// se não houver nos próximos 8 dias (nenhuma faixa aberta).
    pub fn next_open_at(&self, now: i64) -> Option<i64> {
        self.segments(now, now + 8 * DAY_MS)
            .into_iter()
            .find(|(_, _, ok)| *ok)
            .map(|(s, _, _)| s)
    }

    /// Dias úteis (dias locais com faixa de dia útil) que tocam
    /// `[week_start, reset)`: `(total, restantes)`, restantes = os que têm
    /// faixa de dia útil terminando depois de `now`.
    fn workdays(&self, week_start: i64, reset: i64, now: i64) -> (u32, u32) {
        let (mut total, mut left) = (0, 0);
        for day in self.local_day(week_start) - 1..=self.local_day(reset) {
            let occ: Vec<(i64, i64)> = self
                .occurrences(day)
                .filter(|(i, _, _)| self.bands[*i].workday)
                .map(|(_, s, e)| (s, e))
                .filter(|(s, e)| *s < reset && *e > week_start)
                .collect();
            if occ.is_empty() {
                continue;
            }
            total += 1;
            if occ.iter().any(|(_, e)| *e > now) {
                left += 1;
            }
        }
        (total, left)
    }
}

/// Dia da semana do dia local (0 = segunda). 1970-01-01 foi quinta.
fn weekday(day: i64) -> u8 {
    (day + 3).rem_euclid(7) as u8
}

/// Por que uma sessão autônoma nova não pode começar agora.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BlockReason {
    /// A faixa atual é fechada.
    BandClosed { band: String },
    /// Nenhuma faixa cobre o horário atual (conta como fechado).
    NoBand,
    /// Dia útil, depois do corte de início.
    AfterCutoff,
    /// Janela de 5h no teto da faixa ou acima.
    FiveHourOverCap { used: f64, cap: f64 },
    /// Semanal no limite efetivo ou acima.
    WeeklyOverLimit { used: f64, limit: f64 },
    /// Janela sem nenhuma leitura (`five_hour` ou `seven_day`): sem sonda,
    /// não há como saber o uso.
    NoReading { window: String },
}

impl std::fmt::Display for BlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockReason::BandClosed { band } => write!(f, "faixa fechada ({band})"),
            BlockReason::NoBand => write!(f, "fora das faixas"),
            BlockReason::AfterCutoff => write!(f, "depois do corte de início"),
            BlockReason::FiveHourOverCap { used, cap } => {
                write!(f, "5h em {used:.0}%, teto {cap:.0}%")
            }
            BlockReason::WeeklyOverLimit { used, limit } => {
                write!(f, "semanal em {used:.0}%, limite {limit:.1}%")
            }
            BlockReason::NoReading { window } => write!(f, "sem leitura da janela {window}"),
        }
    }
}

/// Como saiu o limite da semanal (para a UI explicar a conta).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WeeklyLimit {
    /// Reset da semanal usado (ms); estimado se a janela renovou.
    pub resets_at: i64,
    /// Dias úteis da semana (`W`) e os que faltam até o reset (`d`).
    pub workdays_total: u32,
    pub workdays_left: u32,
    /// `100 − R × d`.
    pub reserve_limit: f64,
    /// Proporção das horas elegíveis da semana já passadas (0 a 1).
    pub pace: f64,
    /// `R × (W − d) + max(0, T − R × W) × pace`.
    pub pace_limit: f64,
    /// `min(T, reserve_limit, pace_limit)`: até onde a semanal pode ir agora.
    pub effective: f64,
}

/// Resultado de [`evaluate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BudgetDecision {
    /// Uma sessão autônoma nova pode começar agora.
    pub can_start: bool,
    /// Vazio quando `can_start`.
    pub reasons: Vec<BlockReason>,
    /// Faixa atual (rótulo); `None` = fora das faixas.
    pub band: Option<String>,
    /// Teto da 5h na faixa atual; `None` = fechada ou fora das faixas.
    pub five_hour_cap: Option<f64>,
    /// Uso considerado (0 se renovada); `None` = sem leitura.
    pub five_hour_used: Option<f64>,
    pub weekly_used: Option<f64>,
    /// `None` sem leitura da semanal.
    pub weekly: Option<WeeklyLimit>,
    /// Quando o horário volta a permitir iniciar (`now` se já permite);
    /// `None` = a política não tem faixa aberta.
    pub next_open_at: Option<i64>,
    pub now: i64,
}

/// Uso considerado da janela e o reset: renovada conta como 0%.
fn window_used(w: WindowView, now: i64, period: i64) -> Option<(f64, i64)> {
    match w {
        WindowView::Ok {
            used_percentage,
            resets_at,
            ..
        } => Some((used_percentage, resets_at)),
        WindowView::Renewed { resets_at, .. } => {
            // Próximo reset estimado: o último conhecido mais períodos inteiros.
            let k = (now - resets_at).div_euclid(period) + 1;
            Some((0.0, resets_at + k * period))
        }
        WindowView::None => None,
    }
}

/// Limite efetivo da semanal em `now`, com o reset em `resets_at`.
pub fn weekly_limit(policy: &BudgetPolicy, resets_at: i64, now: i64) -> WeeklyLimit {
    let week_start = resets_at - WEEK_MS;
    let (t, r) = (policy.weekly_cap, policy.reserve_per_workday);
    let (w, d) = policy.workdays(week_start, resets_at, now);
    let total = policy.eligible_ms(week_start, resets_at);
    // A faixa elegível atual conta até o fim dela: a cota da faixa fica
    // disponível desde o início.
    let upto = match policy.band_at(now) {
        Some((_, end)) if policy.eligible(now) => end,
        _ => now,
    }
    .clamp(week_start, resets_at);
    let pace = if total == 0 {
        1.0
    } else {
        policy.eligible_ms(week_start, upto) as f64 / total as f64
    };
    let reserve_limit = (100.0 - r * f64::from(d)).max(0.0);
    let autonomous = (t - r * f64::from(w)).max(0.0);
    let pace_limit = r * f64::from(w - d) + autonomous * pace;
    WeeklyLimit {
        resets_at,
        workdays_total: w,
        workdays_left: d,
        reserve_limit,
        pace,
        pace_limit,
        effective: t.min(reserve_limit).min(pace_limit).clamp(0.0, 100.0),
    }
}

/// Avalia a política para a conta em `now` (ms). Pura: o agendador (#183)
/// chama com a visão do plano da conta ([`super::plan_view`]) e o relógio.
pub fn evaluate(policy: &BudgetPolicy, plan: &PlanView, now: i64) -> BudgetDecision {
    let mut reasons = Vec::new();
    let band = policy.band_at(now).map(|(i, _)| &policy.bands[i]);
    let cap = band.and_then(|b| b.five_hour_cap);
    match band {
        None => reasons.push(BlockReason::NoBand),
        Some(b) if b.five_hour_cap.is_none() => reasons.push(BlockReason::BandClosed {
            band: b.label.clone(),
        }),
        Some(_) => {}
    }
    if policy.in_cutoff(now) {
        reasons.push(BlockReason::AfterCutoff);
    }
    let five = window_used(plan.five_hour, now, 5 * 3_600_000);
    match (five, cap) {
        (None, _) => reasons.push(BlockReason::NoReading {
            window: "five_hour".into(),
        }),
        (Some((used, _)), Some(cap)) if used >= cap => {
            reasons.push(BlockReason::FiveHourOverCap { used, cap })
        }
        _ => {}
    }
    let seven = window_used(plan.seven_day, now, WEEK_MS);
    let weekly = seven.map(|(_, resets_at)| weekly_limit(policy, resets_at, now));
    match (seven, weekly) {
        (Some((used, _)), Some(l)) if used >= l.effective => {
            reasons.push(BlockReason::WeeklyOverLimit {
                used,
                limit: l.effective,
            })
        }
        (None, _) => reasons.push(BlockReason::NoReading {
            window: "seven_day".into(),
        }),
        _ => {}
    }
    BudgetDecision {
        can_start: reasons.is_empty(),
        reasons,
        band: band.map(|b| b.label.clone()),
        five_hour_cap: cap,
        five_hour_used: five.map(|(u, _)| u),
        weekly_used: seven.map(|(u, _)| u),
        weekly,
        next_open_at: policy.next_open_at(now),
        now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;
    /// Segunda, 2026-11-09 00:00 em GMT-3 (03:00 UTC).
    const MON: i64 = 1_794_193_200_000;

    /// Instante local GMT-3: dia da semana (0 = segunda desta semana, 7 = a
    /// próxima), hora e minuto.
    fn at(day: i64, h: i64, m: i64) -> i64 {
        MON + day * DAY_MS + h * H + m * MIN_MS
    }

    fn view(five: Option<(f64, i64)>, seven: Option<(f64, i64)>, now: i64) -> PlanView {
        let w = |x: Option<(f64, i64)>| match x {
            None => WindowView::None,
            Some((p, r)) if now < r => WindowView::Ok {
                used_percentage: p,
                resets_at: r,
                read_at: now - H,
            },
            Some((_, r)) => WindowView::Renewed {
                resets_at: r,
                read_at: now - H,
            },
        };
        PlanView {
            five_hour: w(five),
            seven_day: w(seven),
            read_at: Some(now - H),
        }
    }

    #[test]
    fn segunda_de_referencia() {
        let p = BudgetPolicy::default();
        assert_eq!(weekday(p.local_day(MON)), 0);
        assert_eq!(weekday(p.local_day(at(5, 10, 0))), 5);
        // 2026-11-09 é segunda.
        assert_eq!(
            crate::clock::civil_from_days(p.local_day(MON)),
            (2026, 11, 9)
        );
    }

    #[test]
    fn padrao_e_valido() {
        BudgetPolicy::default().validate().unwrap();
        let mut p = BudgetPolicy::default();
        p.bands[0].days = vec![7];
        assert!(p.validate().is_err());
        let p = BudgetPolicy {
            weekly_cap: 120.0,
            ..BudgetPolicy::default()
        };
        assert!(p.validate().is_err());
        let mut p = BudgetPolicy::default();
        p.bands[1].start_min = p.bands[1].end_min;
        assert!(p.validate().is_err());
        assert!(BudgetPolicy {
            bands: vec![],
            ..BudgetPolicy::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn criterio_terca_dez_horas_faixa_fechada() {
        let p = BudgetPolicy::default();
        let now = at(1, 10, 0);
        let d = evaluate(
            &p,
            &view(Some((10.0, now + H)), Some((10.0, at(7, 0, 0))), now),
            now,
        );
        assert!(!d.can_start);
        assert_eq!(
            d.reasons,
            [BlockReason::BandClosed {
                band: "Horário comercial".into()
            }]
        );
        // Abre às 18:00 da terça.
        assert_eq!(d.next_open_at, Some(at(1, 18, 0)));
    }

    #[test]
    fn criterio_sabado_dez_horas_pode_iniciar() {
        let p = BudgetPolicy::default();
        let now = at(5, 10, 0);
        // Reset da semanal na segunda seguinte e no meio da semana seguinte.
        for reset in [at(7, 0, 0), at(9, 12, 0)] {
            let d = evaluate(
                &p,
                &view(Some((40.0, now + H)), Some((30.0, reset)), now),
                now,
            );
            assert!(d.can_start, "reset {reset}: {:?}", d.reasons);
            assert_eq!(d.band.as_deref(), Some("Fim de semana"));
            assert_eq!(d.five_hour_cap, Some(95.0));
            assert_eq!(d.next_open_at, Some(now));
            assert!(d.weekly.unwrap().effective > 30.0);
        }
    }

    #[test]
    fn faixas_e_virada_do_dia() {
        let p = BudgetPolicy::default();
        let band = |t| p.band_at(t).map(|(i, _)| p.bands[i].label.as_str());
        assert_eq!(band(at(0, 7, 59)), Some("Madrugada de dia útil"));
        assert_eq!(band(at(0, 8, 0)), Some("Horário comercial"));
        assert_eq!(band(at(0, 17, 59)), Some("Horário comercial"));
        assert_eq!(band(at(0, 18, 0)), Some("Noite de dia útil"));
        assert_eq!(band(at(0, 23, 59)), Some("Noite de dia útil"));
        assert_eq!(band(at(1, 0, 0)), Some("Madrugada de dia útil"));
        // Sexta à noite ainda é dia útil; à meia-noite vira fim de semana.
        assert_eq!(band(at(4, 23, 59)), Some("Noite de dia útil"));
        assert_eq!(band(at(5, 0, 0)), Some("Fim de semana"));
        assert_eq!(band(at(6, 23, 59)), Some("Fim de semana"));
        assert_eq!(band(at(7, 0, 0)), Some("Madrugada de dia útil"));
    }

    #[test]
    fn faixa_que_atravessa_a_meia_noite() {
        // Uma faixa só, de segunda a sexta, das 22:00 às 06:00 do dia seguinte.
        let p = BudgetPolicy {
            bands: vec![TimeBand {
                label: "Noite".into(),
                days: vec![0, 1, 2, 3, 4],
                start_min: 22 * 60,
                end_min: 6 * 60,
                five_hour_cap: Some(50.0),
                workday: false,
            }],
            workday_cutoff_min: None,
            ..BudgetPolicy::default()
        };
        // Terça 02:00 é a noite que começou na segunda, e termina às 06:00.
        assert_eq!(p.band_at(at(1, 2, 0)), Some((0, at(1, 6, 0))));
        // Sábado 05:00 é a noite que começou na sexta.
        assert!(p.band_at(at(5, 5, 0)).is_some());
        // Segunda 02:00: a noite de domingo não existe.
        assert_eq!(p.band_at(at(0, 2, 0)), None);
        assert_eq!(p.next_open_at(at(0, 2, 0)), Some(at(0, 22, 0)));
    }

    #[test]
    fn corte_de_inicio_nos_dias_uteis() {
        let p = BudgetPolicy::default();
        let five = |now| Some((10.0, now + H));
        let seven = Some((0.0, at(7, 0, 0)));
        // Terça 03:59: noite de dia útil, antes do corte.
        let now = at(1, 3, 59);
        assert!(evaluate(&p, &view(five(now), seven, now), now).can_start);
        // Terça 04:00: corte.
        let now = at(1, 4, 0);
        let d = evaluate(&p, &view(five(now), seven, now), now);
        assert_eq!(d.reasons, [BlockReason::AfterCutoff]);
        // Reabre às 18:00 (corte até 08:00, depois o comercial fechado).
        assert_eq!(d.next_open_at, Some(at(1, 18, 0)));
        // Sábado 05:00 não é dia útil: sem corte.
        let now = at(5, 5, 0);
        let d = evaluate(&p, &view(five(now), seven, now), now);
        assert!(!d.reasons.contains(&BlockReason::AfterCutoff));
        // Sem corte configurado, terça 05:00 pode.
        let sem = BudgetPolicy {
            workday_cutoff_min: None,
            ..BudgetPolicy::default()
        };
        let now = at(1, 5, 0);
        assert!(evaluate(&sem, &view(five(now), seven, now), now).can_start);
    }

    #[test]
    fn teto_da_5h_da_faixa() {
        let p = BudgetPolicy::default();
        let now = at(1, 20, 0);
        let seven = Some((0.0, at(7, 0, 0)));
        let d = evaluate(&p, &view(Some((60.0, now + H)), seven, now), now);
        assert_eq!(
            d.reasons,
            [BlockReason::FiveHourOverCap {
                used: 60.0,
                cap: 60.0
            }]
        );
        // Janela de 5h renovada conta como 0%.
        let d = evaluate(&p, &view(Some((99.0, now - 1)), seven, now), now);
        assert!(d.can_start);
        assert_eq!(d.five_hour_used, Some(0.0));
    }

    #[test]
    fn dias_uteis_restantes_e_reserva() {
        let p = BudgetPolicy::default();
        // Semana de segunda a segunda; segunda 20:00: o comercial de segunda
        // já passou, faltam terça a sexta.
        let l = weekly_limit(&p, at(7, 0, 0), at(0, 20, 0));
        assert_eq!((l.workdays_total, l.workdays_left), (5, 4));
        assert_eq!(l.reserve_limit, 100.0 - 48.0);
        // Sábado: nenhum dia útil até o reset.
        let l = weekly_limit(&p, at(7, 0, 0), at(5, 10, 0));
        assert_eq!(l.workdays_left, 0);
        assert_eq!(l.reserve_limit, 100.0);
    }

    #[test]
    fn reset_da_semanal_no_meio_da_semana() {
        let p = BudgetPolicy::default();
        // Reset quarta 12:00: a semana vai de quarta 12:00 a quarta 12:00 e
        // tem seis dias úteis (as duas quartas tocam a janela).
        let reset = at(9, 12, 0);
        let l = weekly_limit(&p, reset, at(5, 10, 0));
        assert_eq!((l.workdays_total, l.workdays_left), (6, 3));
        assert_eq!(l.reserve_limit, 64.0);
        // R × W = 72: a parte autônoma é 80 − 72 = 8.
        assert!((l.pace_limit - (36.0 + 8.0 * l.pace)).abs() < 1e-9);
        assert!((l.effective - l.pace_limit).abs() < 1e-9);
        // Depois do reset, a janela renovada conta 0% e o reset é estimado
        // uma semana depois.
        let now = reset + H;
        let d = evaluate(
            &p,
            &view(Some((0.0, now + H)), Some((70.0, reset)), now),
            now,
        );
        assert_eq!(d.weekly_used, Some(0.0));
        assert_eq!(d.weekly.unwrap().resets_at, reset + WEEK_MS);
    }

    #[test]
    fn ritmo_cresce_com_as_horas_elegiveis() {
        let p = BudgetPolicy::default();
        let reset = at(7, 0, 0);
        // Horas elegíveis da semana padrão: por dia útil, 00:00–04:00 (o
        // corte fecha 04:00–08:00) e 18:00–24:00; mais o fim de semana.
        let total = 5.0 * 10.0 + 48.0;
        // Segunda 10:00 (fechado): só a madrugada de segunda passou.
        let a = weekly_limit(&p, reset, at(0, 10, 0));
        assert!((a.pace - 4.0 / total).abs() < 1e-9);
        // Segunda 18:00: a noite conta inteira, até 24:00.
        let b = weekly_limit(&p, reset, at(0, 18, 0));
        assert!((b.pace - 10.0 / total).abs() < 1e-9);
        // Sábado 10:00: os cinco dias úteis e o sábado inteiro.
        let c = weekly_limit(&p, reset, at(5, 10, 0));
        assert!((c.pace - 74.0 / total).abs() < 1e-9);
        // Semana de segunda: W = 5, R × W = 60, parte autônoma 20.
        assert!((c.pace_limit - (60.0 + 20.0 * c.pace)).abs() < 1e-9);
        assert!((b.pace_limit - (12.0 + 20.0 * b.pace)).abs() < 1e-9);
        assert!(a.effective < b.effective && b.effective < c.effective);
        // Fim da semana: chega ao teto T.
        let z = weekly_limit(&p, reset, reset - 1);
        assert!((z.pace - 1.0).abs() < 1e-9);
        assert!((z.effective - 80.0).abs() < 1e-9);
    }

    #[test]
    fn semanal_acima_do_limite() {
        let p = BudgetPolicy::default();
        let now = at(0, 20, 0);
        let d = evaluate(
            &p,
            &view(Some((5.0, now + H)), Some((40.0, at(7, 0, 0))), now),
            now,
        );
        let limit = d.weekly.unwrap().effective;
        assert!(limit < 40.0);
        assert_eq!(
            d.reasons,
            [BlockReason::WeeklyOverLimit { used: 40.0, limit }]
        );
    }

    #[test]
    fn sem_leitura_bloqueia() {
        let p = BudgetPolicy::default();
        let now = at(5, 10, 0);
        let d = evaluate(&p, &view(None, None, now), now);
        assert_eq!(
            d.reasons,
            [
                BlockReason::NoReading {
                    window: "five_hour".into()
                },
                BlockReason::NoReading {
                    window: "seven_day".into()
                }
            ]
        );
        assert!(d.weekly.is_none());
    }

    #[test]
    fn fuso_desloca_as_faixas() {
        // A mesma terça 10:00 GMT-3 é 13:00 UTC; com a política em UTC,
        // ainda horário comercial; em UTC+9 são 22:00: noite.
        let p = BudgetPolicy {
            utc_offset_min: 9 * 60,
            ..BudgetPolicy::default()
        };
        let (i, _) = p.band_at(at(1, 10, 0)).unwrap();
        assert_eq!(p.bands[i].label, "Noite de dia útil");
    }

    #[test]
    fn sem_faixa_aberta_nao_tem_proxima() {
        let mut p = BudgetPolicy::default();
        for b in &mut p.bands {
            b.five_hour_cap = None;
        }
        assert_eq!(p.next_open_at(at(1, 10, 0)), None);
        let now = at(1, 10, 0);
        let d = evaluate(
            &p,
            &view(Some((0.0, now + H)), Some((0.0, at(7, 0, 0))), now),
            now,
        );
        assert!(!d.can_start);
    }

    #[test]
    fn fora_das_faixas() {
        let p = BudgetPolicy {
            bands: vec![TimeBand {
                label: "Só sábado".into(),
                days: vec![5],
                start_min: 0,
                end_min: DAY_MIN,
                five_hour_cap: Some(90.0),
                workday: false,
            }],
            ..BudgetPolicy::default()
        };
        let now = at(1, 10, 0);
        let d = evaluate(
            &p,
            &view(Some((0.0, now + H)), Some((0.0, at(7, 0, 0))), now),
            now,
        );
        assert_eq!(d.reasons[0], BlockReason::NoBand);
        assert_eq!(d.next_open_at, Some(at(5, 0, 0)));
        // Sem dia útil, a semana toda é da parte autônoma, no ritmo: na
        // terça nenhuma hora elegível passou (limite 0); no sábado, a faixa
        // inteira conta.
        assert_eq!(d.weekly.unwrap().effective, 0.0);
        let now = at(5, 10, 0);
        let d = evaluate(
            &p,
            &view(Some((0.0, now + H)), Some((30.0, at(7, 0, 0))), now),
            now,
        );
        assert!(d.can_start, "{:?}", d.reasons);
        assert!((d.weekly.unwrap().effective - 80.0).abs() < 1e-9);
    }

    #[test]
    fn resumo_no_horario_e_no_fuso_da_politica() {
        let p = BudgetPolicy::default();
        // Terça 08:59 em GMT-3: ainda não; 09:00: sai, com o dia local.
        assert_eq!(p.digest_due(at(1, 8, 59), None), None);
        let day = p.local_day(at(1, 9, 0));
        assert_eq!(p.digest_due(at(1, 9, 0), None), Some(day));
        // Hub fora do ar às 09:00: sai quando voltar, no mesmo dia.
        assert_eq!(p.digest_due(at(1, 22, 30), None), Some(day));
        // 09:00 em GMT-3 = 12:00 UTC; noutro fuso (UTC), 09:00 UTC é 06:00
        // em GMT-3 e o resumo já está devido.
        let utc = BudgetPolicy {
            utc_offset_min: 0,
            ..BudgetPolicy::default()
        };
        assert!(utc.digest_due(at(1, 6, 0), None).is_some());
        assert!(utc.digest_due(at(1, 5, 59), None).is_none());
        // Horário configurado: 07:30.
        let cedo = BudgetPolicy {
            digest_at_min: Some(7 * 60 + 30),
            ..BudgetPolicy::default()
        };
        assert!(cedo.digest_due(at(1, 7, 29), None).is_none());
        assert!(cedo.digest_due(at(1, 7, 30), None).is_some());
        // Sem horário: sem resumo.
        let sem = BudgetPolicy {
            digest_at_min: None,
            ..BudgetPolicy::default()
        };
        assert!(sem.digest_due(at(1, 12, 0), None).is_none());
        let fora = BudgetPolicy {
            digest_at_min: Some(DAY_MIN),
            ..BudgetPolicy::default()
        };
        assert!(fora.validate().is_err());
    }

    #[test]
    fn resumo_uma_vez_por_dia_util_e_nunca_no_fim_de_semana() {
        let p = BudgetPolicy::default();
        let seg = p.local_day(at(0, 9, 0));
        assert_eq!(p.digest_due(at(0, 9, 0), None), Some(seg));
        // Já enviado hoje: não repete.
        assert_eq!(p.digest_due(at(0, 9, 1), Some(seg)), None);
        assert_eq!(p.digest_due(at(0, 23, 59), Some(seg)), None);
        // Terça: sai de novo.
        assert_eq!(p.digest_due(at(1, 9, 0), Some(seg)), Some(seg + 1));
        // Sábado e domingo: nunca.
        for d in [5, 6] {
            for h in [9, 12, 23] {
                assert_eq!(p.digest_due(at(d, h, 0), Some(seg + 4)), None);
            }
        }
        // Segunda seguinte: sai.
        assert_eq!(p.digest_due(at(7, 9, 0), Some(seg + 4)), Some(seg + 7));
        // Dia útil segue a política: sem faixa de dia útil na quarta, não sai.
        let mut sem_quarta = BudgetPolicy::default();
        sem_quarta.bands[0].days = vec![0, 1, 3, 4];
        assert!(sem_quarta.is_workday(seg + 1));
        assert!(!sem_quarta.is_workday(seg + 2));
        assert_eq!(sem_quarta.digest_due(at(2, 9, 0), Some(seg + 1)), None);
        // Faixa de dia útil no sábado: o resumo continua fora do fim de semana.
        let mut sabado = BudgetPolicy::default();
        sabado.bands[0].days = vec![0, 1, 2, 3, 4, 5];
        assert!(!sabado.is_workday(seg + 5));
        assert_eq!(sabado.digest_due(at(5, 9, 0), Some(seg + 4)), None);
    }

    #[test]
    fn politica_salva_sem_o_horario_do_resumo_vale_09h() {
        let mut v = serde_json::to_value(BudgetPolicy::default()).unwrap();
        v.as_object_mut().unwrap().remove("digest_at_min");
        let p: BudgetPolicy = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(p.digest_at_min, Some(DIGEST_AT_DEFAULT_MIN));
        v["digest_at_min"] = serde_json::Value::Null;
        let p: BudgetPolicy = serde_json::from_value(v).unwrap();
        assert_eq!(p.digest_at_min, None);
    }
}
