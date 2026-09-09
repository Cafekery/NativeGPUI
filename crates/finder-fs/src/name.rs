use std::borrow::Cow;

/// Finder draws a stored `:` as `/`, and a control character other than tab as `?`.
pub fn display(name: &str) -> Cow<'_, str> {
    let mapped = |c: char| c == ':' || (c.is_control() && c != '\t');
    if !name.chars().any(mapped) {
        return Cow::Borrowed(name);
    }
    Cow::Owned(
        name.chars()
            .map(|c| match c {
                ':' => '/',
                c if mapped(c) => '?',
                c => c,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colon_draws_as_the_separator_the_user_typed() {
        assert_eq!(display("colon:in:name.txt"), "colon/in/name.txt");
        assert_eq!(display("plain.txt"), "plain.txt");
    }

    #[test]
    fn a_control_character_keeps_the_name_on_one_line() {
        assert_eq!(display("newline\nin-name.txt"), "newline?in-name.txt");
        assert_eq!(display("bell\u{7}.txt"), "bell?.txt");
        // A tab is whitespace Finder leaves alone; it draws on the row like a space.
        assert_eq!(display("tab\tin-name.txt"), "tab\tin-name.txt");
        assert_eq!(
            display("caf\u{e9} \u{1f600}.txt"),
            "caf\u{e9} \u{1f600}.txt"
        );
    }

    #[test]
    fn a_name_with_nothing_to_map_is_not_copied() {
        assert!(matches!(display("Report.pdf"), Cow::Borrowed(_)));
        assert!(matches!(display("a:b"), Cow::Owned(_)));
    }
}
