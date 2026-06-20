//! SPDX-License-Identifier: MIT OR Apache-2.0

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
pub fn variant_section(input: &str) -> IResult<&str, Vec<String>> {
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
    
    loop {
        // Skip empty lines and comments
        let (new_input, _) = skip_empty_and_comments.parse(input)?;
        input = new_input;
        
        // Try to parse a variant line
        if let Ok((new_input, variant)) = variant_line.parse(input) {
            variants.push(variant.to_string());
            input = new_input;
        } else {
            // No more variant, break
            break;
        }
    }
    
    if variants.is_empty() {
        return Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Many1)));
    }
    
    Ok((input, variants))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_variant_section() {
        // Test successful parsing cases
        assert_eq!(
            variant_section("[variant]\nmyvariant\nmysecondvariant\n"),
            Ok(("", vec!["myvariant".into(), "mysecondvariant".into()]))
        );

        // Test parsing with multiple variant and different names
        assert_eq!(
            variant_section("[variant]\ndebug\nrelease\ntest\nprod\n"),
            Ok(("", vec!["debug".into(), "release".into(), "test".into(), "prod".into()]))
        );

        // Test handling of extra newlines (improved parser leaves trailing newlines)
        assert_eq!(
            variant_section("[variant]\nmyvariant\nmysecondvariant\n\n"),
            Ok(("", vec!["myvariant".into(), "mysecondvariant".into()]))  // Updated expectation
        );

        // Test error cases
        assert!(variant_section("[varian").is_err(), "Should fail on incomplete header");
        assert!(variant_section("[variant]").is_err(), "Should fail without variant");
        assert!(variant_section("[variant]\n").is_err(), "Should fail with empty variant list");
        
        // Test numeric variant (should pass)
        assert_eq!(
            variant_section("[variant]\n123\nabc\nab12\n12ab\n"),
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
            variant_section(complex_input),
            Ok(("", vec![  // Our improved parser consumes trailing newlines
                "debug".into(),
                "release".into(),
                "test".into(),
                "benchmark".into(),
                "profiling".into()
            ]))
        );
    }
}
