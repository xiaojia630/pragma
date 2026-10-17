//! YAML frontmatter: the optional `---` block at the top of a markdown file.

/// Splits `---\n<yaml>---\n<body>` into its parts. A file without frontmatter is all body.
pub fn split(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            return (Some(&rest[..offset]), &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    (None, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_from_body() {
        assert_eq!(
            split("---\na: 1\n---\n# Body\n"),
            (Some("a: 1\n"), "# Body\n")
        );
        assert_eq!(split("---\n---\nbody"), (Some(""), "body"));
        assert_eq!(split("# No frontmatter\n"), (None, "# No frontmatter\n"));
        assert_eq!(split("---\nnever closed\n"), (None, "---\nnever closed\n"));
    }
}
