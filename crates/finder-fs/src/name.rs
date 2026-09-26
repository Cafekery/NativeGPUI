use std::borrow::Cow;

/// A name as Finder draws it: `:` as the `/` the user typed, a control character as `?`, and a
/// direction override closed before the extension, so `\u{202e}gnp.txt` can't pass for a
/// `.png`.
pub fn display(name: &str) -> Cow<'_, str> {
    let mapped = |c: char| c == ':' || (c.is_control() && c != '\t');
    let open = name.chars().fold(0usize, |open, c| match c {
        '\u{202a}' | '\u{202b}' | '\u{202d}' | '\u{202e}' => open + 1,
        '\u{202c}' => open.saturating_sub(1),
        _ => open,
    });
    if open == 0 && !name.chars().any(mapped) {
        return Cow::Borrowed(name);
    }
    let (stem, extension) = match name.rfind('.').filter(|&dot| dot > 0 && open > 0) {
        Some(dot) => name.split_at(dot),
        None => (name, ""),
    };
    let map = |c: char| match c {
        ':' => '/',
        c if mapped(c) => '?',
        c => c,
    };
    Cow::Owned(
        stem.chars()
            .map(map)
            .chain(std::iter::repeat_n('\u{202c}', open))
            .chain(extension.chars().map(map))
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
        assert_eq!(display("tab\tin-name.txt"), "tab\tin-name.txt");
        assert_eq!(
            display("caf\u{e9} \u{1f600}.txt"),
            "caf\u{e9} \u{1f600}.txt"
        );
    }

    #[test]
    fn a_direction_override_is_closed_before_the_extension() {
        assert_eq!(display("\u{202e}gnp.txt"), "\u{202e}gnp\u{202c}.txt");
        assert_eq!(display("\u{202e}a\u{202c}b.txt"), "\u{202e}a\u{202c}b.txt");
        assert_eq!(
            display("\u{202d}\u{202e}x"),
            "\u{202d}\u{202e}x\u{202c}\u{202c}"
        );
    }

    #[test]
    fn a_name_with_nothing_to_map_is_not_copied() {
        assert!(matches!(display("Report.pdf"), Cow::Borrowed(_)));
        assert!(matches!(display("a:b"), Cow::Owned(_)));
    }
}
