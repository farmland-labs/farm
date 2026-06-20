//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Cache section parser for Farmland configuration files
//!
//! The `[cache]` section defines global cache configuration including
//! platform-specific keys, size limits, and TTL settings.

use nom::{
    IResult,
    bytes::complete::{tag, take_while1, take_till},
    character::complete::{char, line_ending, space0},
    sequence::{delimited, terminated},
    branch::alt,
    Parser,
};
use std::collections::HashMap;
use crate::vendor::parse::plan::CacheConfig;

/// `cache_section` parses a complete cache configuration section.
/// 
/// # Examples
/// ```text
/// [cache]
/// max_size: 10GB
/// ttl: 30d
/// env_key:
///     FARM_PLATFORM
///     RUST_TARGET
/// env_command:
///     RUSTC_VERSION: rustc --version
///     NODE_VERSION: node --version
/// ```
pub fn cache_section(input: &str) -> IResult<&str, CacheConfig> {
    use nom::error::Error;
    use super::general::comment_line;

    // Parse [cache] header
    let mut cache_section_header = delimited(
        char('['),
        tag("cache"),
        terminated(char(']'), line_ending::<&str, Error<&str>>)
    );

    // Parse the header
    let (mut remaining, _) = cache_section_header.parse(input)?;

    let mut config = CacheConfig::default();

    loop {
        // Skip empty lines and comments
        loop {
            if let Ok((new_input, _)) = line_ending::<&str, Error<&str>>(remaining) {
                remaining = new_input;
                continue;
            }
            if let Ok((new_input, _)) = comment_line(remaining) {
                remaining = new_input;
                continue;
            }
            break;
        }

        // Check for end or next section
        if remaining.is_empty() || remaining.starts_with('[') {
            break;
        }

        // Try to parse max_size
        if let Ok((new_input, size)) = max_size_line(remaining) {
            config.max_size = Some(size);
            remaining = new_input;
            continue;
        }

        // Try to parse ttl
        if let Ok((new_input, ttl)) = ttl_line(remaining) {
            config.ttl = Some(ttl);
            remaining = new_input;
            continue;
        }

        // Try to parse env_key (multi-line)
        if let Ok((new_input, keys)) = env_key_section(remaining) {
            config.env_key = keys;
            remaining = new_input;
            continue;
        }

        // Try to parse env_command (multi-line)
        if let Ok((new_input, commands)) = env_command_section(remaining) {
            config.env_command = commands;
            remaining = new_input;
            continue;
        }

        // Unknown line in cache section - skip it
        // This allows forward compatibility with new fields
        if let Ok((new_input, _)) = skip_unknown_line(remaining) {
            remaining = new_input;
            continue;
        }

        break;
    }

    Ok((remaining, config))
}

/// Parse `max_size: 10GB` line
fn max_size_line(input: &str) -> IResult<&str, String> {
    use nom::error::Error;
    
    let (input, _) = tag("max_size:")(input)?;
    let (input, _) = space0(input)?;
    let (input, value) = take_while1(|c: char| c.is_alphanumeric())(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = line_ending::<&str, Error<&str>>(input)?;
    
    Ok((input, value.to_string()))
}

/// Parse `ttl: 30d` line
fn ttl_line(input: &str) -> IResult<&str, String> {
    use nom::error::Error;
    
    let (input, _) = tag("ttl:")(input)?;
    let (input, _) = space0(input)?;
    let (input, value) = take_while1(|c: char| c.is_alphanumeric())(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = line_ending::<&str, Error<&str>>(input)?;
    
    Ok((input, value.to_string()))
}

/// Parse env_key section:
/// ```text
/// env_key:
///     FARM_PLATFORM
///     RUST_TARGET
/// ```
fn env_key_section(input: &str) -> IResult<&str, Vec<String>> {
    use nom::error::Error;
    
    let (remaining, _) = tag("env_key:")(input)?;
    let (remaining, _) = space0(remaining)?;
    let (mut remaining, _) = line_ending::<&str, Error<&str>>(remaining)?;
    
    let mut keys = Vec::new();
    
    loop {
        // Skip empty lines and comments within indented block
        loop {
            if remaining.starts_with("    ") || remaining.starts_with("\t") {
                let trimmed = remaining.trim_start();
                if trimmed.starts_with('#') {
                    // Indented comment
                    if let Some(newline_pos) = remaining.find('\n') {
                        remaining = &remaining[newline_pos + 1..];
                        continue;
                    }
                }
            }
            if let Ok((new_input, _)) = line_ending::<&str, Error<&str>>(remaining) {
                if new_input.starts_with("    ") || new_input.starts_with("\t") {
                    // Empty line followed by indented content - skip
                    remaining = new_input;
                    continue;
                }
            }
            break;
        }
        
        // Check for indentation (at least 4 spaces or tab)
        if !remaining.starts_with("    ") && !remaining.starts_with("\t") {
            break;
        }
        
        // Skip indentation
        let (new_input, _) = alt((tag("    "), tag("\t"))).parse(remaining)?;
        
        // Skip any additional whitespace
        let (new_input, _) = space0(new_input)?;
        
        // Parse key name (alphanumeric + underscore)
        let is_env_key_char = |c: char| c.is_alphanumeric() || c == '_';
        let (new_input, key) = take_while1(is_env_key_char)(new_input)?;
        
        // Skip trailing whitespace and comment
        let (new_input, _) = space0(new_input)?;
        let new_input = if new_input.starts_with('#') {
            // Skip to end of line
            if let Some(newline_pos) = new_input.find('\n') {
                &new_input[newline_pos..]
            } else {
                ""
            }
        } else {
            new_input
        };
        
        let (new_input, _) = line_ending::<&str, Error<&str>>(new_input)?;
        
        keys.push(key.to_string());
        remaining = new_input;
    }
    
    Ok((remaining, keys))
}

/// Parse env_command section:
/// ```text
/// env_command:
///     RUSTC_VERSION: rustc --version
///     NODE_VERSION: node --version
/// ```
fn env_command_section(input: &str) -> IResult<&str, HashMap<String, String>> {
    use nom::error::Error;
    
    let (remaining, _) = tag("env_command:")(input)?;
    let (remaining, _) = space0(remaining)?;
    let (mut remaining, _) = line_ending::<&str, Error<&str>>(remaining)?;
    
    let mut commands = HashMap::new();
    
    loop {
        // Skip empty lines within indented block
        loop {
            if remaining.starts_with("    ") || remaining.starts_with("\t") {
                let trimmed = remaining.trim_start();
                if trimmed.starts_with('#') {
                    // Indented comment
                    if let Some(newline_pos) = remaining.find('\n') {
                        remaining = &remaining[newline_pos + 1..];
                        continue;
                    }
                }
            }
            if let Ok((new_input, _)) = line_ending::<&str, Error<&str>>(remaining) {
                if new_input.starts_with("    ") || new_input.starts_with("\t") {
                    remaining = new_input;
                    continue;
                }
            }
            break;
        }
        
        // Check for indentation (at least 4 spaces or tab)
        if !remaining.starts_with("    ") && !remaining.starts_with("\t") {
            break;
        }
        
        // Skip indentation
        let (new_input, _) = alt((tag("    "), tag("\t"))).parse(remaining)?;
        
        // Skip any additional whitespace
        let (new_input, _) = space0(new_input)?;
        
        // Parse key name
        let is_env_key_char = |c: char| c.is_alphanumeric() || c == '_';
        let (new_input, key) = take_while1(is_env_key_char)(new_input)?;
        
        // Parse colon
        let (new_input, _) = char(':')(new_input)?;
        let (new_input, _) = space0(new_input)?;
        
        // Parse command (rest of line, minus trailing comment)
        let (new_input, command) = take_till(|c| c == '\n' || c == '\r')(new_input)?;
        
        // Strip trailing comment if present
        let command = if let Some(hash_pos) = command.find('#') {
            command[..hash_pos].trim()
        } else {
            command.trim()
        };
        
        let (new_input, _) = line_ending::<&str, Error<&str>>(new_input)?;
        
        commands.insert(key.to_string(), command.to_string());
        remaining = new_input;
    }
    
    Ok((remaining, commands))
}

/// Skip an unknown line (for forward compatibility)
fn skip_unknown_line(input: &str) -> IResult<&str, ()> {
    use nom::error::Error;
    
    let (input, _) = take_till(|c| c == '\n' || c == '\r')(input)?;
    let (input, _) = line_ending::<&str, Error<&str>>(input)?;
    
    Ok((input, ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_section_full() {
        let input = r#"[cache]
max_size: 10GB
ttl: 30d
env_key:
    FARM_PLATFORM
    RUST_TARGET
env_command:
    RUSTC_VERSION: rustc --version
    NODE_VERSION: node --version
"#;
        let (remaining, config) = cache_section(input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(config.max_size, Some("10GB".to_string()));
        assert_eq!(config.ttl, Some("30d".to_string()));
        assert_eq!(config.env_key, vec!["FARM_PLATFORM", "RUST_TARGET"]);
        assert_eq!(config.env_command.get("RUSTC_VERSION"), Some(&"rustc --version".to_string()));
        assert_eq!(config.env_command.get("NODE_VERSION"), Some(&"node --version".to_string()));
    }

    #[test]
    fn test_cache_section_minimal() {
        let input = "[cache]\nmax_size: 5GB\n";
        let (remaining, config) = cache_section(input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(config.max_size, Some("5GB".to_string()));
        assert!(config.env_key.is_empty());
        assert!(config.env_command.is_empty());
    }

    #[test]
    fn test_cache_section_env_key_only() {
        let input = r#"[cache]
env_key:
    CC
    CFLAGS
"#;
        let (remaining, config) = cache_section(input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(config.env_key, vec!["CC", "CFLAGS"]);
    }

    #[test]
    fn test_cache_section_with_comments() {
        let input = r#"[cache]
# Maximum cache size
max_size: 10GB
env_key:
    FARM_PLATFORM      # Platform identifier
    CC                 # Compiler
"#;
        let (remaining, config) = cache_section(input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(config.max_size, Some("10GB".to_string()));
        assert_eq!(config.env_key, vec!["FARM_PLATFORM", "CC"]);
    }

    #[test]
    fn test_cache_section_followed_by_operation() {
        let input = r#"[cache]
max_size: 10GB
[operation.build]
"#;
        let (remaining, config) = cache_section(input).unwrap();
        assert_eq!(remaining, "[operation.build]\n");
        assert_eq!(config.max_size, Some("10GB".to_string()));
    }

    #[test]
    fn test_env_command_with_pipes() {
        let input = r#"[cache]
env_command:
    GCC_VERSION: gcc --version 2>&1 | head -1
"#;
        let (remaining, config) = cache_section(input).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(
            config.env_command.get("GCC_VERSION"),
            Some(&"gcc --version 2>&1 | head -1".to_string())
        );
    }
}
