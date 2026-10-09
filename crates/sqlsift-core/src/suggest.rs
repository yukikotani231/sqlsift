//! "Did you mean ...?" suggestions for misspelled names

/// Find the candidate most similar to `name` (for "did you mean" suggestions)
pub(crate) fn find_similar_name(
    candidates: impl IntoIterator<Item = String>,
    name: &str,
) -> Option<String> {
    find_most_similar(candidates, |c| c.as_str(), name)
}

/// Find the candidate whose `key` is most similar to `name`, ignoring case.
///
/// A candidate is similar if it is within roughly one edit per three characters of
/// `name`, or if `name` (at least three characters) is a prefix of it. On a tie the
/// earliest candidate wins.
pub(crate) fn find_most_similar<T>(
    candidates: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> &str,
    name: &str,
) -> Option<T> {
    let name_lower = name.to_lowercase();
    let max_distance = name_lower.chars().count().div_ceil(3).clamp(1, 3);
    let mut best_match: Option<(usize, T)> = None;

    for candidate in candidates {
        let candidate_lower = key(&candidate).to_lowercase();
        let distance = edit_distance(&name_lower, &candidate_lower);
        // A name that is a prefix of the candidate (`author` -> `author_id`) is similar
        let is_prefix = name_lower.chars().count() >= 3 && candidate_lower.starts_with(&name_lower);

        // Allow roughly one edit per three characters (at least 1, at most 3)
        if (is_prefix || distance <= max_distance)
            && best_match
                .as_ref()
                .map_or(true, |(best, _)| distance < *best)
        {
            best_match = Some((distance, candidate));
        }
    }

    best_match.map(|(_, candidate)| candidate)
}

/// Damerau-Levenshtein distance (optimal string alignment variant), allowing insertion,
/// deletion, substitution, and transposition of adjacent characters.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut dp = vec![vec![0; n + 1]; m + 1];

    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = i;
    }
    for (j, val) in dp[0].iter_mut().enumerate() {
        *val = j;
    }

    for i in 1..=m {
        for j in 1..=n {
            let cost = usize::from(a_chars[i - 1] != b_chars[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);

            if i > 1
                && j > 1
                && a_chars[i - 1] == b_chars[j - 2]
                && a_chars[i - 2] == b_chars[j - 1]
            {
                dp[i][j] = dp[i][j].min(dp[i - 2][j - 2] + 1);
            }
        }
    }

    dp[m][n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_names_only() {
        let names = || ["ambiguous-column", "column-not-found"].map(String::from);
        assert_eq!(
            find_similar_name(names(), "ambigous-column").as_deref(),
            Some("ambiguous-column")
        );
        assert_eq!(find_similar_name(names(), "zzz"), None);
    }

    #[test]
    fn matches_on_the_key() {
        let tables = [("billing", "charges"), ("public", "users")];
        let found = find_most_similar(tables, |t| t.1, "charge");
        assert_eq!(found, Some(("billing", "charges")));
    }

    #[test]
    fn transpositions_count_as_single_edit() {
        assert_eq!(edit_distance("kidn", "kind"), 1);
        assert_eq!(edit_distance("dya", "day"), 1);

        let candidates = ["id", "kind", "day"].map(String::from);
        assert_eq!(
            find_similar_name(candidates.clone(), "kidn").as_deref(),
            Some("kind")
        );
        assert_eq!(find_similar_name(candidates, "dya").as_deref(), Some("day"));
    }
}
