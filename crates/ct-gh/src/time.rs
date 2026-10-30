//! Datas da API do GitHub (`2026-10-15T22:47:50Z`) ⇄ milissegundos Unix.

use ct_core::civil_from_days;

const DAY_MS: i64 = 86_400_000;

/// Converte `AAAA-MM-DDTHH:MM:SSZ` (UTC, formato da API) em ms; `None` se
/// o texto não estiver nesse formato.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = &s[r];
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())
            .flatten()
    };
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, se) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * DAY_MS + ((h * 60 + mi) * 60 + se) * 1000)
}

/// Início (00:00:00Z) do dia UTC de `ms`, no formato da API.
pub fn day_start_utc(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(DAY_MS));
    format!("{y:04}-{m:02}-{d:02}T00:00:00Z")
}

// Algoritmo de Howard Hinnant (calendário gregoriano proléptico); o inverso
// está em `ct_core::civil_from_days`.
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
    use super::*;

    #[test]
    fn datas_conhecidas() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        assert_eq!(
            parse_rfc3339("2024-02-29T12:00:00Z"),
            Some(1_709_208_000_000)
        );
        assert_eq!(
            parse_rfc3339("2026-12-31T23:59:59Z"),
            parse_rfc3339("2027-01-01T00:00:00Z").map(|v| v - 1000)
        );
    }

    #[test]
    fn formato_invalido() {
        for s in [
            "",
            "2026-10-15",
            "2026-10-15T22:47:50+00:00",
            "2026-13-01T00:00:00Z",
            "2026-1a-01T00:00:00Z",
            "2026-10-15 22:47:50Z",
        ] {
            assert_eq!(parse_rfc3339(s), None, "{s}");
        }
    }

    #[test]
    fn inicio_do_dia() {
        assert_eq!(day_start_utc(1_709_208_000_000), "2024-02-29T00:00:00Z");
        assert_eq!(day_start_utc(0), "1970-01-01T00:00:00Z");
        let fim_do_ano = parse_rfc3339("2026-12-31T23:59:59Z").unwrap();
        assert_eq!(day_start_utc(fim_do_ano), "2026-12-31T00:00:00Z");
        assert_eq!(day_start_utc(fim_do_ano + 1000), "2027-01-01T00:00:00Z");
        // Ida e volta em cada dia de um ano bissexto.
        let inicio = parse_rfc3339("2028-01-01T00:00:00Z").unwrap();
        for d in 0..366 {
            let ms = inicio + d * DAY_MS;
            assert_eq!(parse_rfc3339(&day_start_utc(ms + 3_600_000)), Some(ms));
        }
    }
}
