//! Subsequence fuzzy match for filterable pickers (models, providers, commands).

fn is_sep(c: char) -> bool {
    matches!(c, '/' | '-' | '_' | '.' | ' ' | ':')
}

/// Case-insensitive subsequence score; `0` means no match.
pub fn score(haystack: &str, needle: &str) -> i32 {
    if needle.is_empty() {
        return 1;
    }
    let h: Vec<char> = haystack.to_lowercase().chars().collect();
    let n: Vec<char> = needle.to_lowercase().chars().collect();
    let mut hi = 0usize;
    let mut ni = 0usize;
    let mut total = 0i32;
    let mut prev: Option<usize> = None;
    let mut run = 0i32;
    while hi < h.len() && ni < n.len() {
        if h[hi] == n[ni] {
            let mut pts = 10;
            if hi == 0 || is_sep(h[hi - 1]) {
                pts += 15;
            }
            if hi > 0 && prev == Some(hi - 1) {
                run += 1;
                pts += run * 5;
            } else {
                run = 0;
            }
            total += pts;
            prev = Some(hi);
            ni += 1;
        }
        hi += 1;
    }
    if ni < n.len() {
        0
    } else {
        total
    }
}

/// Items with a positive fuzzy score for `needle`, highest score first (stable tie-break).
pub fn filter_ranked<'a>(items: &'a [String], needle: &str) -> Vec<&'a str> {
    if needle.is_empty() {
        return items.iter().map(String::as_str).collect();
    }
    let mut hits: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let s = score(item, needle);
            (s > 0).then_some((i, s))
        })
        .collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hits.into_iter().map(|(i, _)| items[i].as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_needle_matches_trivially() {
        assert!(score("anything", "") > 0);
    }

    #[test]
    fn typo_needle_matches_nemotron() {
        assert!(score("nvidia/nemotron-70b-instruct", "nemtron") > 0);
        assert!(score("nvidia/nemotron-70b-instruct", "nemo") > 0);
    }

    #[test]
    fn unrelated_gets_zero() {
        assert_eq!(score("alpha/beta-gamma", "nemtron"), 0);
    }

    #[test]
    fn filter_ranked_prefers_better_match() {
        let items = vec![
            "other/thing".into(),
            "nvidia/nemotron-70b".into(),
            "nvidia/nemotron-mini".into(),
        ];
        let shown = filter_ranked(&items, "nemtron");
        assert_eq!(shown.len(), 2);
        assert!(shown[0].contains("nemotron"));
        assert!(shown[1].contains("nemotron"));
        assert!(!shown.contains(&"other/thing"));
    }

    #[test]
    fn filter_ranked_empty_needle_preserves_order() {
        let items = vec!["c".into(), "a".into(), "b".into()];
        assert_eq!(filter_ranked(&items, ""), vec!["c", "a", "b"]);
    }
}
