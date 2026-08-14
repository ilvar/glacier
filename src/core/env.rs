//! Environment lookup with a documented precedence order.
//!
//! Every optional feature reads its configuration from a `LEGACY_`-prefixed
//! variable first and a conventional vendor name second. That ordering is
//! deliberate: a user who already exports `OPENAI_API_KEY` for other tools
//! gets a working setup for free, while anyone who wants this program
//! pointed somewhere else can override it without disturbing the rest of
//! their shell.

/// The first of `names` that is set to a non-empty value.
///
/// An empty or whitespace-only variable is treated as unset, because
/// `export OPENAI_API_KEY=` is a common way to *disable* a key and should
/// not be mistaken for configuring one.
pub fn first(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(value) = std::env::var(name) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::first;

    /// Unique names per test: cargo runs tests in one process, so shared
    /// variable names would make these depend on each other.
    #[test]
    fn prefers_the_first_name_that_is_set() {
        std::env::set_var("LEGACY_TEST_PRIMARY_A", "primary");
        std::env::set_var("LEGACY_TEST_FALLBACK_A", "fallback");
        assert_eq!(
            first(&["LEGACY_TEST_PRIMARY_A", "LEGACY_TEST_FALLBACK_A"]).as_deref(),
            Some("primary")
        );
        std::env::remove_var("LEGACY_TEST_PRIMARY_A");
        std::env::remove_var("LEGACY_TEST_FALLBACK_A");
    }

    #[test]
    fn falls_back_when_the_first_is_absent() {
        std::env::remove_var("LEGACY_TEST_PRIMARY_B");
        std::env::set_var("LEGACY_TEST_FALLBACK_B", "fallback");
        assert_eq!(
            first(&["LEGACY_TEST_PRIMARY_B", "LEGACY_TEST_FALLBACK_B"]).as_deref(),
            Some("fallback")
        );
        std::env::remove_var("LEGACY_TEST_FALLBACK_B");
    }

    #[test]
    fn an_empty_value_counts_as_unset() {
        // `export KEY=` is how people turn a key off; it must not be read
        // as an empty key that then fails confusingly at the API.
        std::env::set_var("LEGACY_TEST_PRIMARY_C", "   ");
        std::env::set_var("LEGACY_TEST_FALLBACK_C", "fallback");
        assert_eq!(
            first(&["LEGACY_TEST_PRIMARY_C", "LEGACY_TEST_FALLBACK_C"]).as_deref(),
            Some("fallback")
        );
        std::env::remove_var("LEGACY_TEST_PRIMARY_C");
        std::env::remove_var("LEGACY_TEST_FALLBACK_C");
    }

    #[test]
    fn returns_none_when_nothing_is_set() {
        std::env::remove_var("LEGACY_TEST_PRIMARY_D");
        std::env::remove_var("LEGACY_TEST_FALLBACK_D");
        assert_eq!(
            first(&["LEGACY_TEST_PRIMARY_D", "LEGACY_TEST_FALLBACK_D"]),
            None
        );
        assert_eq!(first(&[]), None);
    }
}
