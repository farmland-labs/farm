//! SPDX-License-Identifier: MIT OR Apache-2.0

use super::lenient::{self, Skipped};
use nom::{
    IResult,
    bytes::complete::tag,
    character::complete::{char, line_ending},
    sequence::terminated,
    sequence::delimited,
    multi::many0,
    combinator::map,
    branch::alt,
    Parser,
    bytes::complete::take_while1,
};

/// `variant_section` parses a complete section of variant definition. The variant section has an header enclosed in square brackets.
/// The body comprises a permutation of `variant_line`.
/// 
/// # Examples
/// ```text
///     [variant]
///     build
///     test    =>  ["build", "test"]
/// ```
///
/// Unrecognised lines are skipped and returned as [`Skipped`]; an empty
/// section is valid.
pub(crate) fn variant_section(input: &str) -> IResult<&str, (Vec<String>, Vec<Skipped<'_>>)> {
    use nom::error::Error;
    use super::general::comment_line;

    let mut variant_section_header = delimited(
        char('['), 
        tag("variant"), 
        terminated(char(']'), line_ending::<&str, Error<&str>>)
    );
    
    // Parse each variant line, ensuring it contains valid identifier characters
    let is_valid_identifier = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let variant = take_while1(is_valid_identifier);
    let mut variant_line = terminated(variant, line_ending::<&str, Error<&str>>);
    
    // Helper to skip empty lines and comments
    let mut skip_empty_and_comments = many0(alt((
        map(line_ending::<&str, Error<&str>>, |_| ()),
        map(comment_line, |_| ())
    )));
    
    // Parse the header followed by variant lines with optional empty lines/comments
    let (mut input, _) = variant_section_header.parse(input)?;
    
    let mut variants = Vec::new();
    let mut skipped = Vec::new();
    
    loop {
        // Skip empty lines and comments
        let (new_input, _) = skip_empty_and_comments.parse(input)?;
        input = new_input;
        if let Some(new_input) = lenient::skip_blank_line(input) {
            input = new_input;
            continue;
        }
        
        // Try to parse a variant line
        if let Ok((new_input, variant)) = variant_line.parse(input) {
            variants.push(variant.to_string());
            input = new_input;
            continue;
        }

        // No more variants: next section, decorator or EOF
        if lenient::at_section_end(input) {
            break;
        }

        input = lenient::skip_unrecognised(input, "in [variant] section", &mut skipped);
    }
    
    Ok((input, (variants, skipped)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `variant_section` without the skipped lines, for the assertions below.
    fn variants(input: &str) -> IResult<&str, Vec<String>> {
        variant_section(input).map(|(rest, (variants, _))| (rest, variants))
    }

    #[test]
    fn test_variant_section() {
        // Test successful parsing cases
        assert_eq!(
            variants("[variant]\nmyvariant\nmysecondvariant\n"),
            Ok(("", vec!["myvariant".into(), "mysecondvariant".into()]))
        );

        // Test parsing with multiple variant and different names
        assert_eq!(
            variants("[variant]\ndebug\nrelease\ntest\nprod\n"),
            Ok(("", vec!["debug".into(), "release".into(), "test".into(), "prod".into()]))
        );

        // Test handling of extra newlines (improved parser leaves trailing newlines)
        assert_eq!(
            variants("[variant]\nmyvariant\nmysecondvariant\n\n"),
            Ok(("", vec!["myvariant".into(), "mysecondvariant".into()]))  // Updated expectation
        );

        // Test error cases
        assert!(variant_section("[varian").is_err(), "Should fail on incomplete header");
        assert!(variant_section("[variant]").is_err(), "Should fail without header line ending");
        assert_eq!(variants("[variant]\n"), Ok(("", vec![])), "Empty section is valid");
        
        // Test numeric variant (should pass)
        assert_eq!(
            variants("[variant]\n123\nabc\nab12\n12ab\n"),
            Ok(("", vec!["123".into(), "abc".into(), "ab12".into(), "12ab".into()]))
        );
        
        // Test with complex realistic variant
        let complex_input = "\
[variant]
debug
release
test
benchmark
profiling
\n";
        assert_eq!(
            variants(complex_input),
            Ok(("", vec![  // Our improved parser consumes trailing newlines
                "debug".into(),
                "release".into(),
                "test".into(),
                "benchmark".into(),
                "profiling".into()
            ]))
        );
    }

    #[test]
    fn test_variant_section_skips_unrecognised_line() {
        let (rest, (variants, skipped)) = variant_section("[variant]\ndebug\nnot valid\nrelease\n").unwrap();
        assert_eq!(rest, "");
        assert_eq!(variants, vec!["debug", "release"]);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].message, "unrecognised line 'not valid' in [variant] section");
    }
}
