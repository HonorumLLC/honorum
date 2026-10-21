//! Text normalization for provider strings. Adapters apply it to fields a
//! provider publishes in capitals, so every school's titles read alike.

/// Rewrites an all-caps provider string such as `INTRO TO C++` as
/// `Intro To C++`. A word starts after anything but a letter, digit, or
/// underscore.
pub(crate) fn title_case(s: &str) -> String {
    let mut prev = ' ';
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let start = !prev.is_alphabetic() && !prev.is_numeric() && prev != '_';
        prev = c;
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::title_case;

    #[test]
    fn title_case_matches_word_boundaries() {
        for (input, want) in [
            ("INTRO TO C++", "Intro To C++"),
            ("O'NEIL, MARY-KATE", "O'Neil, Mary-Kate"),
            ("CALCULUS II (HONORS)", "Calculus Ii (Honors)"),
            ("", ""),
            ("already Mixed case_two", "Already Mixed Case_two"),
        ] {
            assert_eq!(title_case(input), want, "title_case({input:?})");
        }
    }
}
