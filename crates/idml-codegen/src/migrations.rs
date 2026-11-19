//! Reviewed exact-endpoint migrations. These are not current-model projections.
use std::fmt::Write as _;

pub fn rust(text: &str) -> Result<String, String> {
    let mut out = format!(
        "{}\nuse crate::migration::{{MigrationRule, MigrationAction}};\nuse crate::DomVersion;\npub fn rules() -> Vec<MigrationRule> {{ vec![\n",
        crate::emit::HEADER
    );
    let mut ids = std::collections::BTreeSet::new();
    for (i, row) in text.lines().enumerate() {
        if row.is_empty() || row.starts_with('#') || row.starts_with("id\tfrom\t") {
            continue;
        }
        let error = |m: &str| format!("schema-migrations.tsv:{}: {m}", i + 1);
        let columns: Vec<_> = row.split('\t').collect();
        let [id, from, to, path, action, argument, lossy, reason, evidence] = columns.as_slice() else {
            return Err(error("expected nine columns"));
        };
        if id.is_empty() || !ids.insert(*id) || path.split('/').any(str::is_empty) || reason.is_empty() || evidence.is_empty() {
            return Err(error("unique ID, path, reason and evidence are required"));
        }
        let dom = |s: &str| -> Result<(u16, u16), String> {
            let (major, minor) = s.split_once('.').ok_or_else(|| error("invalid endpoint"))?;
            Ok((major.parse().map_err(|_| error("invalid endpoint"))?, minor.parse().map_err(|_| error("invalid endpoint"))?))
        };
        let (a, b) = dom(from)?;
        let (c, d) = dom(to)?;
        if (a, b) == (c, d) {
            return Err(error("migration endpoints must differ"));
        }
        let is_lossy = match *lossy {
            "true" => true,
            "false" => false,
            _ => return Err(error("lossy must be true or false")),
        };
        let strings = |items: Vec<&str>| items.iter().map(|s| format!("{s:?}.into()")).collect::<Vec<_>>().join(",");
        let operation = match (*action, argument.split('|').collect::<Vec<_>>().as_slice()) {
            ("wrap", [s]) => {
                let wrappers: Option<Vec<_>> = s.split('/').map(|s| s.strip_prefix('<')?.strip_suffix('>')).collect();
                format!("MigrationAction::Wrap(vec![{}])", strings(wrappers.ok_or_else(|| error("expected <A>/<B> wrappers"))?))
            }
            ("map", [name, from, to]) if is_lossy => {
                format!("MigrationAction::MapValue {{ name: {name:?}.into(), from: {from:?}.into(), to: {to:?}.into() }}")
            }
            _ => return Err(error("unsupported action or unacknowledged semantic loss")),
        };
        let _ = writeln!(
            out,
            "MigrationRule {{ id: {id:?}.into(), from: DomVersion {{ major:{a}, minor:{b} }}, to: DomVersion {{ major:{c}, minor:{d} }}, path: vec![{}], action: {operation}, lossy: {is_lossy}, reason: {reason:?}.into(), evidence: {evidence:?}.into() }},",
            strings(path.split('/').collect())
        );
    }
    out.push_str("] }\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_implicit_loss_and_unscoped_rules() {
        assert!(rust("x\t7.5\t8.0\tP\tmap\ta|b|c\tfalse\treason\tevidence").is_err());
        assert!(rust("x\t8.0\t8.0\tP\twrap\t<A>\tfalse\treason\tevidence").is_err());
        assert!(rust("x\t6.0\t7.0\tP/C\twrap\t<A>\tfalse\treason\tevidence").is_ok());
    }
}
