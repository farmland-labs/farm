//! SPDX-License-Identifier: MIT OR Apache-2.0

use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::combinator::eof;
use nom::sequence::terminated;
use nom::character::complete::{not_line_ending, line_ending};

/// Parse a line of text, terminated by either a newline or EOF.
/// This allows the last line in a file to not have a trailing newline.
pub fn line_until_end(input: &str) -> IResult<&str, &str> {
    terminated(not_line_ending, alt((line_ending, eof))).parse(input)
}
