//! A1 references only. Evaluation, 3D refs, names, unions, and dynamic arrays stay unread.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaRef {
    pub display: String,
    pub external: bool,
}

pub fn formula_refs(input: &str) -> Vec<FormulaRef> {
    let formula = input.trim().trim_start_matches('=');
    let mut refs = Vec::new();
    let mut index = 0;
    while index < formula.len() && refs.len() < 32 {
        if formula[index..].starts_with('"') {
            index = skip_string(formula, index);
            continue;
        }
        if let Some((end, reference)) = parse_at(formula, index) {
            refs.push(reference);
            index = end;
            continue;
        }
        index += formula[index..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    refs
}

pub fn formula_is_external(formula: &str, refs: &[FormulaRef]) -> bool {
    refs.iter().any(|reference| reference.external) || bracket_outside_string(formula)
}

fn parse_at(formula: &str, start: usize) -> Option<(usize, FormulaRef)> {
    let rest = formula.get(start..)?;
    if rest.starts_with('\'') {
        return parse_quoted(formula, start);
    }
    if rest.starts_with('[') {
        return parse_bracket(formula, start);
    }
    if let Some(found) = parse_unquoted_sheet(formula, start) {
        return Some(found);
    }
    let end = parse_cell_range(formula, start)?;
    if !boundary(formula, end) {
        return None;
    }
    Some((
        end,
        FormulaRef {
            display: formula[start..end].to_string(),
            external: false,
        },
    ))
}

fn parse_quoted(formula: &str, start: usize) -> Option<(usize, FormulaRef)> {
    let mut index = start + 1;
    let mut external = false;
    while index < formula.len() {
        let ch = formula[index..].chars().next()?;
        if ch == '\'' {
            if formula[index + 1..].starts_with('\'') {
                index += 2;
                continue;
            }
            index += 1;
            break;
        }
        if ch == '[' {
            external = true;
        }
        index += ch.len_utf8();
    }
    if !formula[index..].starts_with('!') {
        return None;
    }
    let end = parse_cell_range(formula, index + 1)?;
    if !boundary(formula, end) {
        return None;
    }
    Some((
        end,
        FormulaRef {
            display: formula[start..end].to_string(),
            external,
        },
    ))
}

fn parse_bracket(formula: &str, start: usize) -> Option<(usize, FormulaRef)> {
    let close = formula[start + 1..].find(']')?;
    let after = start + 1 + close + 1;
    let bang = formula[after..].find('!')?;
    if bang == 0 {
        return None;
    }
    let end = parse_cell_range(formula, after + bang + 1)?;
    if !boundary(formula, end) {
        return None;
    }
    Some((
        end,
        FormulaRef {
            display: formula[start..end].to_string(),
            external: true,
        },
    ))
}

fn parse_unquoted_sheet(formula: &str, start: usize) -> Option<(usize, FormulaRef)> {
    let first = formula[start..].chars().next()?;
    if first.is_ascii_digit() || first == '$' || is_stop(first) {
        return None;
    }
    let mut index = start;
    while index < formula.len() {
        let ch = formula[index..].chars().next()?;
        if ch == '!' {
            break;
        }
        if is_stop(ch) || ch == '$' || ch == '[' || ch == ']' {
            return None;
        }
        index += ch.len_utf8();
    }
    if index == start || !formula[index..].starts_with('!') {
        return None;
    }
    let end = parse_cell_range(formula, index + 1)?;
    if !boundary(formula, end) {
        return None;
    }
    Some((
        end,
        FormulaRef {
            display: formula[start..end].to_string(),
            external: false,
        },
    ))
}

fn parse_cell_range(formula: &str, start: usize) -> Option<usize> {
    let end = parse_cell(formula, start)?;
    if formula[end..].starts_with(':') {
        return parse_cell(formula, end + 1);
    }
    Some(end)
}

fn parse_cell(formula: &str, start: usize) -> Option<usize> {
    let bytes = formula.as_bytes();
    let mut index = start;
    if bytes.get(index) == Some(&b'$') {
        index += 1;
    }
    let column = index;
    while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
        index += 1;
    }
    if index == column || index - column > 3 {
        return None;
    }
    if bytes.get(index) == Some(&b'$') {
        index += 1;
    }
    let row = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == row || index - row > 7 {
        return None;
    }
    Some(index)
}

fn boundary(formula: &str, end: usize) -> bool {
    match formula[end..].chars().next() {
        Some(ch) if ch.is_ascii_alphanumeric() => false,
        _ => true,
    }
}

fn is_stop(ch: char) -> bool {
    matches!(
        ch,
        '+' | '-'
            | '*'
            | '/'
            | '^'
            | '&'
            | '='
            | '<'
            | '>'
            | '('
            | ')'
            | ','
            | ':'
            | '!'
            | ' '
            | '\t'
            | '"'
            | '\''
    )
}

fn skip_string(formula: &str, start: usize) -> usize {
    let mut index = start + 1;
    while index < formula.len() {
        if formula[index..].starts_with("\"\"") {
            index += 2;
            continue;
        }
        if formula[index..].starts_with('"') {
            return index + 1;
        }
        index += formula[index..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(1);
    }
    formula.len()
}

fn bracket_outside_string(formula: &str) -> bool {
    let mut index = 0;
    let mut quoted = false;
    while index < formula.len() {
        let ch = formula[index..].chars().next().unwrap_or(' ');
        if ch == '"' {
            if quoted && formula[index + 1..].starts_with('"') {
                index += 2;
                continue;
            }
            quoted = !quoted;
        } else if ch == '[' && !quoted {
            return true;
        }
        index += ch.len_utf8();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn displays(formula: &str) -> Vec<String> {
        formula_refs(formula)
            .into_iter()
            .map(|reference| reference.display)
            .collect()
    }

    #[test]
    fn reads_sheet_absolute_and_range_forms() {
        assert_eq!(displays("Prices!A1"), vec!["Prices!A1"]);
        assert_eq!(displays("'Sheet Name'!$A$1"), vec!["'Sheet Name'!$A$1"]);
        assert_eq!(displays("A1:B2"), vec!["A1:B2"]);
        assert_eq!(displays("Sheet1!$A$1:$B$2"), vec!["Sheet1!$A$1:$B$2"]);
        assert_eq!(displays("价格!A1"), vec!["价格!A1"]);
        assert_eq!(displays("=SUM(A1)"), vec!["A1"]);
    }

    #[test]
    fn keeps_external_workbook_refs() {
        let refs = formula_refs("'[Other.xlsx]Sheet'!A1");
        assert_eq!(refs.len(), 1);
        assert!(refs[0].external);
        assert!(formula_is_external(
            "[1]Sheet!A1",
            &formula_refs("[1]Sheet!A1")
        ));
        assert!(!formula_is_external(
            "Prices!A1",
            &formula_refs("Prices!A1")
        ));
    }
}
