//! Shared local fuzzy matching; no product entities or I/O.
/// Multi-term, Unicode-safe subsequence matching. Exact/prefix/contiguous title
/// matches lead, then close subsequences; secondary fields keep a lower weight.
pub fn score_field(field: &str, term: &str) -> Option<usize> {
    if field == term {
        return Some(0);
    }
    if field.starts_with(term) {
        return Some(
            5 + field
                .chars()
                .count()
                .saturating_sub(term.chars().count())
                .min(20),
        );
    }
    if let Some(at) = field.find(term) {
        return Some(30 + field[..at].chars().count().min(50));
    }
    let mut needle = term.chars();
    let mut wanted = needle.next()?;
    let mut first = None;
    for (i, ch) in field.chars().enumerate() {
        if ch == wanted {
            first.get_or_insert(i);
            if let Some(next) = needle.next() {
                wanted = next;
            } else {
                return Some(
                    90 + first.unwrap_or(0).min(50)
                        + (i - first.unwrap_or(0) + 1 - term.chars().count()).min(200),
                );
            }
        }
    }
    None
}

/// All whitespace-separated terms must match; the first field has priority.
pub fn score(fields: &[String], query: &str) -> Option<usize> {
    let fields = fields.iter().map(|f| f.to_lowercase()).collect::<Vec<_>>();
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .try_fold(0, |total, term| {
            fields
                .iter()
                .enumerate()
                .filter_map(|(i, f)| score_field(f, &term).map(|n| n + if i == 0 { 0 } else { 60 }))
                .min()
                .map(|n| total + n)
        })
}
