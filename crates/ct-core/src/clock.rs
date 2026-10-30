use std::time::{SystemTime, UNIX_EPOCH};

/// Milissegundos desde a época Unix.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `(ano, mês, dia)` do dia `z` contado desde 1970-01-01 (algoritmo de
/// Howard Hinnant, calendário gregoriano proléptico).
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `AAAAMMDD-HHMM` em UTC (nomes de snapshot).
pub fn utc_stamp(ms: i64) -> String {
    const DAY_MS: i64 = 86_400_000;
    let (y, m, d) = civil_from_days(ms.div_euclid(DAY_MS));
    let min = ms.rem_euclid(DAY_MS) / 60_000;
    format!("{y:04}{m:02}{d:02}-{:02}{:02}", min / 60, min % 60)
}

/// Data ISO 8601 com hora (`2026-10-28T10:15:30.000-0300` do Jira,
/// `2026-10-28T13:15:30.123Z` do GitLab; também `-03:00`, com ou sem os
/// milissegundos) → ms Unix. `None` fora desse formato.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |t: &str| -> Option<i64> {
        (!t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()))
            .then(|| t.parse().ok())
            .flatten()
    };
    let (y, mo, d) = (num(&s[0..4])?, num(&s[5..7])?, num(&s[8..10])?);
    let (h, mi, se) = (num(&s[11..13])?, num(&s[14..16])?, num(&s[17..19])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = &s[19..];
    let mut ms = 0;
    if let Some(r) = rest.strip_prefix('.') {
        let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        let frac = &r[..end];
        // Só os três primeiros dígitos (ms).
        let mut digits: String = frac.chars().take(3).collect();
        while digits.len() < 3 {
            digits.push('0');
        }
        ms = num(&digits)?;
        rest = &r[end..];
    }
    let offset_min = match rest {
        "Z" | "" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hhmm = rest[1..].replace(':', "");
            if hhmm.len() != 4 {
                return None;
            }
            sign * (num(&hhmm[0..2])? * 60 + num(&hhmm[2..4])?)
        }
    };
    let local = days_from_civil(y, mo, d) * 86_400_000 + ((h * 60 + mi) * 60 + se) * 1000 + ms;
    Some(local - offset_min * 60_000)
}

// Algoritmo de Howard Hinnant (calendário gregoriano proléptico); o inverso
// é [`civil_from_days`].
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    #[test]
    fn carimbo_utc() {
        assert_eq!(super::utc_stamp(0), "19700101-0000");
        // 2026-10-16T13:45:30Z
        assert_eq!(super::utc_stamp(1_792_158_330_000), "20261016-1345");
        assert_eq!(super::civil_from_days(-1), (1969, 12, 31));
    }
}

/// Relógio lógico híbrido (HLC) num `i64`: milissegundos nos bits altos e um
/// contador nos 16 bits baixos. A ordem numérica é a ordem causal; um valor
/// cabe numa coluna INTEGER do SQLite.
pub mod hlc {
    /// Bits do contador lógico.
    pub const COUNTER_BITS: u32 = 16;

    /// HLC do instante `ms`, com contador zero.
    pub fn from_ms(ms: i64) -> i64 {
        ms << COUNTER_BITS
    }

    /// Parte física (milissegundos) do HLC.
    pub fn to_ms(hlc: i64) -> i64 {
        hlc >> COUNTER_BITS
    }

    /// Próximo HLC de um evento local: avança o relógio físico ou, se ele não
    /// andou (ou voltou), o contador do último valor.
    pub fn tick(last: i64, now_ms: i64) -> i64 {
        from_ms(now_ms).max(last + 1)
    }

    /// Próximo HLC ao receber `remote` de outro ambiente.
    pub fn recv(last: i64, remote: i64, now_ms: i64) -> i64 {
        from_ms(now_ms).max(last.max(remote) + 1)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn tick_e_monotonico_mesmo_com_relogio_voltando() {
            let a = tick(0, 1_000);
            assert_eq!(a, from_ms(1_000));
            let b = tick(a, 1_000);
            assert_eq!(b, a + 1);
            let c = tick(b, 900);
            assert!(c > b);
            assert_eq!(to_ms(c), 1_000);
            assert_eq!(tick(c, 2_000), from_ms(2_000));
        }

        #[test]
        fn recv_passa_do_remoto() {
            let remoto = from_ms(5_000) + 3;
            let r = recv(from_ms(1_000), remoto, 2_000);
            assert_eq!(r, remoto + 1);
        }
    }
}
