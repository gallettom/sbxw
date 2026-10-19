//! Reading the column tables `sbx` prints.
//!
//! Columns are located by where their headers start — two or more spaces end a
//! header, one space is part of it ("RULE ID", "LAST SEEN") — and each row is
//! cut at those offsets. Splitting rows on whitespace would shred values that
//! contain spaces ("corporate policy", "inactive — superseded").

pub fn header_spans(line: &str) -> Vec<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(String, usize)> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == ' ' {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while i < chars.len() {
            if chars[i] == ' ' {
                // Two spaces in a row close the field; a single one is part of
                // a multi-word header.
                if chars.get(i + 1).is_none_or(|c| *c == ' ') {
                    break;
                }
            } else {
                end = i + 1;
            }
            i += 1;
        }
        out.push((chars[start..end].iter().collect(), start));
    }

    // Only an ALL-CAPS row is a header. Without this check the first *data*
    // line of a non-table output would be mistaken for one.
    let caps = out.iter().all(|(name, _)| {
        name.chars().any(|c| c.is_ascii_uppercase())
            && !name.contains(|c: char| c.is_ascii_lowercase())
    });
    if caps {
        out
    } else {
        Vec::new()
    }
}

/// Cut `line` at the given character offsets, trimming each cell. A value wider
/// than its column bleeds into the next one; we accept that over dropping it,
/// since sbx pads its tables to fit.
pub fn slice_at(line: &str, starts: &[usize]) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(chars.len());
            let start = start.min(chars.len());
            let end = end.min(chars.len()).max(start);
            chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

/// A parsed table: header names, and one cell per column per row. Empty for
/// anything that is not a table, so callers fall back to the raw text rather
/// than claiming there are no rows.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn parse(raw: &str) -> Table {
    let mut lines = raw.lines().skip_while(|l| l.trim().is_empty());
    let Some(header) = lines.next() else {
        return Table::default();
    };
    let spans = header_spans(header);
    if spans.len() < 2 {
        return Table::default();
    }
    let starts: Vec<usize> = spans.iter().map(|(_, s)| *s).collect();
    let rows = lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| slice_at(l, &starts))
        // A row entirely in the first column is a separator or a note.
        .filter(|cells| cells.iter().skip(1).any(|c| !c.is_empty()))
        .collect();
    Table {
        columns: spans.into_iter().map(|(n, _)| n).collect(),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_word_headers_and_values_with_spaces_stay_whole() {
        let raw = "TYPE      RESOURCE            ACTION   ORIGIN            STATUS\n\
                   domain    *.npmjs.org         allow    local policy      active\n\
                   domain    telemetry.acme.com  deny     corporate policy  active\n";
        let t = parse(raw);
        assert_eq!(t.columns, ["TYPE", "RESOURCE", "ACTION", "ORIGIN", "STATUS"]);
        assert_eq!(t.rows[1][3], "corporate policy");
    }

    #[test]
    fn prose_is_not_a_table() {
        assert_eq!(parse("No rules configured.\n"), Table::default());
        assert_eq!(parse(""), Table::default());
        assert!(header_spans("RULE ID   LAST SEEN").iter().map(|(n, _)| n.as_str()).eq(["RULE ID", "LAST SEEN"]));
    }
}
