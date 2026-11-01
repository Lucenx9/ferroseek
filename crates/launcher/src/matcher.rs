// SPDX-License-Identifier: GPL-3.0-or-later
//! Scoring of short labels (application names, keywords) against a query.
//!
//! Every whitespace-separated query token must match the candidate, in any
//! order. A token scores by its best kind of match: whole label, label
//! prefix, word prefix, substring, then an in-order subsequence that
//! prefers word starts. File names are ranked by the daemon, not here.

/// Score of one token against one label, or `None` if it does not match.
fn token_score(token: &str, label: &str) -> Option<u32> {
    if token.is_empty() {
        return Some(0);
    }
    if label == token {
        return Some(1000);
    }
    if let Some(rest) = label.strip_prefix(token) {
        // Shorter remainders rank higher: "fire" prefers "firefox" to
        // "firewall configuration".
        return Some(900 - (rest.chars().count().min(100) as u32));
    }
    let mut best: Option<u32> = None;
    for (i, _) in label.match_indices(token) {
        let at_word = label[..i].chars().next_back().is_none_or(is_separator);
        let s = if at_word { 800 } else { 600 };
        best = Some(best.map_or(s, |b| b.max(s)));
        if at_word {
            break;
        }
    }
    if best.is_some() {
        return best;
    }
    subsequence_score(token, label)
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | ':' | '(' | ')')
}

/// In-order subsequence match. Each matched character scores more on a
/// word start and when it directly follows the previous match.
fn subsequence_score(token: &str, label: &str) -> Option<u32> {
    let chars: Vec<char> = label.chars().collect();
    let mut pos = 0usize;
    let mut score = 0u32;
    let mut prev: Option<usize> = None;
    for t in token.chars() {
        let found = (pos..chars.len()).find(|&i| chars[i] == t)?;
        let word_start = found == 0 || is_separator(chars[found - 1]);
        score += if word_start { 30 } else { 5 };
        if prev.is_some_and(|p| p + 1 == found) {
            score += 15;
        }
        prev = Some(found);
        pos = found + 1;
    }
    // Long spread-out matches are weak; keep every subsequence below a
    // plain substring.
    Some((100 + score).min(500))
}

/// Lower-cased, whitespace-split tokens of a query.
pub fn tokens(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Scores `labels` (already lower-cased) against query `tokens`. Labels are
/// ordered by importance: the first (the name) counts fully, later ones
/// (generic name, keywords, …) are discounted. Returns `None` unless every
/// token matches some label.
pub fn score(tokens: &[String], labels: &[(&str, u32)]) -> Option<u32> {
    if tokens.is_empty() {
        return None;
    }
    let mut total = 0u32;
    for t in tokens {
        let best = labels.iter().filter_map(|(label, weight)| token_score(t, label).map(|s| s * weight / 100)).max()?;
        total += best;
    }
    Some(total / tokens.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(q: &str, label: &str) -> Option<u32> {
        score(&tokens(q), &[(label, 100)])
    }

    #[test]
    fn match_kinds_are_ordered() {
        let exact = s("firefox", "firefox").unwrap();
        let prefix = s("fire", "firefox").unwrap();
        let word = s("code", "visual studio code").unwrap();
        let sub = s("efo", "firefox").unwrap();
        let subseq = s("ffx", "firefox").unwrap();
        assert!(
            exact > prefix && prefix > word && word > sub && sub > subseq,
            "{exact} {prefix} {word} {sub} {subseq}"
        );
    }

    #[test]
    fn shorter_prefix_completion_wins() {
        assert!(s("fire", "firefox") > s("fire", "firewall configuration"));
    }

    #[test]
    fn every_token_must_match_in_any_order() {
        assert!(s("code visual", "visual studio code").is_some());
        assert!(s("code emacs", "visual studio code").is_none());
        assert!(s("", "anything").is_none());
    }

    #[test]
    fn subsequence_prefers_word_starts() {
        assert!(s("vsc", "visual studio code") > s("vsc", "envs-check"));
        assert!(s("xyz", "firefox").is_none());
    }

    #[test]
    fn secondary_labels_are_discounted() {
        let by_name = score(&tokens("term"), &[("terminal", 100), ("shell", 60)]).unwrap();
        let by_keyword = score(&tokens("term"), &[("konsole", 100), ("terminal", 60)]).unwrap();
        assert!(by_name > by_keyword);
    }

    #[test]
    fn unicode_is_handled() {
        assert!(s("àgo", "àgora").is_some());
        assert!(s("日本", "日本語 editor").is_some());
    }
}
