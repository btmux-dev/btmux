//! Smart case, as in Vim and fzf: a query with an uppercase letter matches
//! case. The rule is the frontend's (`lib/smartCase.ts`).

/// Whether `query` matches case: lowercasing changes it.
pub fn matches_case(query: &str) -> bool {
    query.to_lowercase() != query
}

#[cfg(test)]
mod tests {
    use super::matches_case;

    #[test]
    fn uppercase_and_titlecase_letters_match_case() {
        assert!(!matches_case("main.rs"));
        assert!(matches_case("Main"));
        // U+01C5, titlecase but not uppercase.
        assert!(matches_case("ǅ"));
    }
}
