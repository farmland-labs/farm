//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Error handling for the Farmland parser
//! 
//! This module provides detailed error information with line numbers, context,
//! and helpful suggestions for fixing parsing errors.

use std::fmt;
use nom::error::{Error as NomError, ErrorKind};
use nom::{Err as NomErr, IResult};

/// Detailed parse error with location information and helpful context
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    /// The line number where the error occurred (1-based)
    pub line: usize,
    /// The column number where the error occurred (1-based)  
    pub column: usize,
    /// The specific error kind
    pub kind: ParseErrorKind,
    /// The problematic input snippet for context
    pub context: String,
    /// Optional suggestion for fixing the error
    pub suggestion: Option<String>,
}

/// Different types of parse errors with specific context
#[derive(Debug, Clone, PartialEq)]
pub enum ParseErrorKind {
    /// Missing or invalid version line
    InvalidVersion { found: Option<String> },
    /// Invalid section header
    InvalidSectionHeader { found: String },
    /// Invalid operation name
    InvalidOperationName { found: String },
    /// Invalid variant name
    InvalidVariantName { found: String },
    /// Missing required field in a section
    MissingRequiredField { field: String, section: String },
    /// Invalid field value
    InvalidFieldValue { field: String, found: String, expected: String },
    /// Unknown field in a section
    UnknownField { field: String, section: String },
    /// Circular dependency detected
    CircularDependency { operations: Vec<String> },
    /// Unknown operation referenced in dependency
    UnknownDependency { operation: String, referenced_by: String },
    /// Duplicate operation definition
    DuplicateOperation { operation: String },
    /// Invalid syntax
    InvalidSyntax { expected: String, found: String },
    /// Unexpected end of file
    UnexpectedEof { expected: String },
    /// Generic parsing error with context
    Generic { message: String },
}

impl ParseError {
    /// Create a new parse error at the given position
    pub fn new(input: &str, position: &str, kind: ParseErrorKind) -> Self {
        let (line, column) = calculate_line_column(input, position);
        let context = extract_context(input, position, 40);
        let suggestion = generate_suggestion(&kind);
        
        Self {
            line,
            column,
            kind,
            context,
            suggestion,
        }
    }
    
    /// Create an error for invalid version
    pub fn invalid_version(input: &str, position: &str, found: Option<String>) -> Self {
        Self::new(input, position, ParseErrorKind::InvalidVersion { found })
    }
    
    /// Create an error for invalid section header
    pub fn invalid_section_header(input: &str, position: &str, found: String) -> Self {
        Self::new(input, position, ParseErrorKind::InvalidSectionHeader { found })
    }
    
    /// Create an error for missing required field
    pub fn missing_required_field(input: &str, position: &str, field: String, section: String) -> Self {
        Self::new(input, position, ParseErrorKind::MissingRequiredField { field, section })
    }
    
    /// Create an error for invalid syntax
    pub fn invalid_syntax(input: &str, position: &str, expected: String, found: String) -> Self {
        Self::new(input, position, ParseErrorKind::InvalidSyntax { expected, found })
    }
    
    /// Create an error for unexpected EOF
    pub fn unexpected_eof(input: &str, position: &str, expected: String) -> Self {
        Self::new(input, position, ParseErrorKind::UnexpectedEof { expected })
    }
    
    /// Create a generic parse error
    pub fn generic(input: &str, position: &str, message: String) -> Self {
        Self::new(input, position, ParseErrorKind::Generic { message })
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Parse error at line {}, column {}:", self.line, self.column)?;
        writeln!(f, "  {}", self.kind)?;
        
        if !self.context.is_empty() {
            writeln!(f, "  Context: {}", self.context)?;
        }
        
        if let Some(ref suggestion) = self.suggestion {
            writeln!(f, "  Suggestion: {}", suggestion)?;
        }
        
        Ok(())
    }
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseErrorKind::InvalidVersion { found } => {
                match found {
                    Some(version) => write!(f, "Invalid version '{}'. Only version 1 is supported.", version),
                    None => write!(f, "Missing version line. Farmfile must start with 'version: 1'"),
                }
            }
            ParseErrorKind::InvalidSectionHeader { found } => {
                write!(f, "Invalid section header '[{}]'. Expected [variant] or [operation.name]", found)
            }
            ParseErrorKind::InvalidOperationName { found } => {
                write!(f, "Invalid operation name '{}'. Operation names must contain only letters, numbers, underscores, and hyphens", found)
            }
            ParseErrorKind::InvalidVariantName { found } => {
                write!(f, "Invalid variant name '{}'. Variant names must contain only letters, numbers, underscores, and hyphens", found)
            }
            ParseErrorKind::MissingRequiredField { field, section } => {
                write!(f, "Missing required field '{}' in [{}] section", field, section)
            }
            ParseErrorKind::InvalidFieldValue { field, found, expected } => {
                write!(f, "Invalid value '{}' for field '{}'. Expected {}", found, field, expected)
            }
            ParseErrorKind::UnknownField { field, section } => {
                write!(f, "Unknown field '{}' in [{}] section", field, section)
            }
            ParseErrorKind::CircularDependency { operations } => {
                write!(f, "Circular dependency detected: {}", operations.join(" -> "))
            }
            ParseErrorKind::UnknownDependency { operation, referenced_by } => {
                write!(f, "Operation '{}' depends on unknown operation '{}'", referenced_by, operation)
            }
            ParseErrorKind::DuplicateOperation { operation } => {
                write!(f, "Duplicate operation definition: '{}'", operation)
            }
            ParseErrorKind::InvalidSyntax { expected, found } => {
                write!(f, "Invalid syntax. Expected {} but found '{}'", expected, found)
            }
            ParseErrorKind::UnexpectedEof { expected } => {
                write!(f, "Unexpected end of file. Expected {}", expected)
            }
            ParseErrorKind::Generic { message } => {
                write!(f, "{}", message)
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Calculate line and column numbers for a position in the input
fn calculate_line_column(input: &str, position: &str) -> (usize, usize) {
    let offset = position.as_ptr() as isize - input.as_ptr() as isize;
    if offset < 0 || offset as usize > input.len() {
        return (1, 1); // Default to start if calculation fails
    }
    
    let prefix = &input[..offset as usize];
    let line = prefix.matches('\n').count() + 1;
    let column = prefix.rfind('\n')
        .map(|last_newline| offset as usize - last_newline)
        .unwrap_or(offset as usize + 1);
    
    (line, column)
}

/// Extract context around the error position for display
fn extract_context(input: &str, position: &str, max_len: usize) -> String {
    let offset = position.as_ptr() as isize - input.as_ptr() as isize;
    if offset < 0 || offset as usize > input.len() {
        return String::new();
    }
    
    let start = (offset as usize).saturating_sub(max_len / 2);
    let end = std::cmp::min(input.len(), offset as usize + max_len / 2);
    
    let context = &input[start..end];
    
    // Clean up the context - replace newlines with ↵ symbol
    context.replace('\n', "↵").replace('\r', "")
}

/// Generate helpful suggestions based on the error kind
fn generate_suggestion(kind: &ParseErrorKind) -> Option<String> {
    match kind {
        ParseErrorKind::InvalidVersion { .. } => {
            Some("Add 'version: 1' as the first line of your Farmfile".to_string())
        }
        ParseErrorKind::InvalidSectionHeader { found } => {
            if found.starts_with("operation") {
                Some("Try '[operation.your_operation_name]' for operation sections".to_string())
            } else {
                Some("Valid sections are [variant] and [operation.name]".to_string())
            }
        }
        ParseErrorKind::InvalidOperationName { .. } => {
            Some("Use only letters, numbers, underscores, and hyphens in operation names".to_string())
        }
        ParseErrorKind::InvalidVariantName { .. } => {
            Some("Use only letters, numbers, underscores, and hyphens in variant names".to_string())
        }
        ParseErrorKind::MissingRequiredField { field, section } => {
            match (field.as_str(), section.as_str()) {
                ("work", _) => Some("Add a 'work:' line with the command to execute".to_string()),
                _ => Some(format!("Add the required '{}:' field to this section", field)),
            }
        }
        ParseErrorKind::UnknownField { field: _, section } => {
            match section.as_str() {
                s if s.starts_with("operation") => {
                    Some("Valid fields for operations: work, after, variant".to_string())
                }
                "variant" => {
                    Some("variant section should only contain variant names, one per line".to_string())
                }
                _ => None,
            }
        }
        ParseErrorKind::CircularDependency { .. } => {
            Some("Remove circular dependencies by reordering your operations".to_string())
        }
        ParseErrorKind::UnknownDependency { operation, .. } => {
            Some(format!("Define operation '{}' or remove it from the 'after:' list", operation))
        }
        ParseErrorKind::DuplicateOperation { .. } => {
            Some("Use different operation names or add variant to distinguish them".to_string())
        }
        ParseErrorKind::InvalidSyntax { expected, .. } => {
            Some(format!("Expected {}", expected))
        }
        ParseErrorKind::UnexpectedEof { expected } => {
            Some(format!("Add {} before the end of the file", expected))
        }
        _ => None,
    }
}

/// Convert nom error to our custom ParseError
pub fn convert_nom_error(input: &str, err: NomErr<NomError<&str>>) -> ParseError {
    match err {
        NomErr::Incomplete(_) => {
            ParseError::unexpected_eof(input, input, "more input".to_string())
        }
        NomErr::Error(e) | NomErr::Failure(e) => {
            let message = match e.code {
                ErrorKind::Tag => "expected specific tag or keyword".to_string(),
                ErrorKind::Char => "expected specific character".to_string(),
                ErrorKind::Alpha => "expected alphabetic character".to_string(),
                ErrorKind::Digit => "expected digit".to_string(),
                ErrorKind::Eof => "unexpected end of input".to_string(),
                _ => format!("parsing error: {:?}", e.code),
            };
            ParseError::generic(input, e.input, message)
        }
    }
}

/// Helper trait to convert nom IResult to Result with better errors
pub trait ParseResultExt<'a, T> {
    fn with_context(self, input: &'a str, context: &str) -> Result<(&'a str, T), ParseError>;
}

impl<'a, T> ParseResultExt<'a, T> for IResult<&'a str, T> {
    fn with_context(self, input: &'a str, context: &str) -> Result<(&'a str, T), ParseError> {
        self.map_err(|e| {
            let mut parse_err = convert_nom_error(input, e);
            if let ParseErrorKind::Generic { ref mut message } = parse_err.kind {
                *message = format!("{}: {}", context, message);
            }
            parse_err
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_column_calculation() {
        let input = "line 1\nline 2\nline 3";
        let position = &input[7..]; // Points to "line 2\nline 3"
        let (line, column) = calculate_line_column(input, position);
        assert_eq!(line, 2);
        assert_eq!(column, 1);
    }

    #[test]
    fn test_context_extraction() {
        let input = "this is a long line of text that should be truncated";
        let position = &input[10..]; // Points to "long line..."
        let context = extract_context(input, position, 20);
        assert!(context.len() <= 20);
        assert!(context.contains("long"));
    }

    #[test]
    fn test_error_display() {
        let error = ParseError::invalid_version("version: 2\n", "version: 2\n", Some("2".to_string()));
        let display = format!("{}", error);
        assert!(display.contains("line 1"));
        assert!(display.contains("Invalid version"));
        assert!(display.contains("Only version 1"));
    }
}
