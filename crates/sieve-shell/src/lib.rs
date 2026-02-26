#![forbid(unsafe_code)]

use sieve_types::{CommandKnowledge, CommandSegment, CompositionOperator};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellAnalysis {
    pub knowledge: CommandKnowledge,
    pub segments: Vec<CommandSegment>,
    pub unsupported_constructs: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ShellAnalysisError {
    #[error("failed to parse shell input: {0}")]
    Parse(String),
}

pub trait ShellAnalyzer: Send + Sync {
    fn analyze_shell_lc_script(&self, script: &str) -> Result<ShellAnalysis, ShellAnalysisError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct BasicShellAnalyzer;

impl ShellAnalyzer for BasicShellAnalyzer {
    fn analyze_shell_lc_script(&self, script: &str) -> Result<ShellAnalysis, ShellAnalysisError> {
        let split = split_by_supported_operators(script)?;
        if split.command_texts.is_empty() {
            return Ok(ShellAnalysis {
                knowledge: CommandKnowledge::Unknown,
                segments: Vec::new(),
                unsupported_constructs: Vec::new(),
            });
        }

        let mut unsupported = split.unsupported_constructs;
        let mut segments = Vec::with_capacity(split.command_texts.len());

        for (index, command_text) in split.command_texts.into_iter().enumerate() {
            let argv = tokenize_command(command_text, &mut unsupported)?;
            if argv.is_empty() {
                return Ok(ShellAnalysis {
                    knowledge: CommandKnowledge::Unknown,
                    segments: Vec::new(),
                    unsupported_constructs: dedupe_preserve_order(unsupported),
                });
            }

            let operator_before = if index == 0 {
                None
            } else {
                Some(split.operators[index - 1])
            };
            segments.push(CommandSegment {
                argv,
                operator_before,
            });
        }

        let unsupported = dedupe_preserve_order(unsupported);
        if !unsupported.is_empty() {
            return Ok(ShellAnalysis {
                knowledge: CommandKnowledge::Uncertain,
                segments: Vec::new(),
                unsupported_constructs: unsupported,
            });
        }

        Ok(ShellAnalysis {
            knowledge: CommandKnowledge::Known,
            segments,
            unsupported_constructs: Vec::new(),
        })
    }
}

#[derive(Debug)]
struct SplitResult<'a> {
    command_texts: Vec<&'a str>,
    operators: Vec<CompositionOperator>,
    unsupported_constructs: Vec<String>,
}

fn split_by_supported_operators(script: &str) -> Result<SplitResult<'_>, ShellAnalysisError> {
    let mut command_texts = Vec::new();
    let mut operators = Vec::new();
    let mut unsupported_constructs = Vec::new();

    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    let bytes = script.as_bytes();
    let mut idx = 0;
    let mut start = 0;

    while idx < bytes.len() {
        let ch = bytes[idx] as char;

        if escaped {
            escaped = false;
            idx += 1;
            continue;
        }

        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            idx += 1;
            continue;
        }

        if in_double {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            } else if ch == '$' || ch == '`' {
                push_construct(&mut unsupported_constructs, "substitution_or_expansion");
            }
            idx += 1;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '\'' => in_single = true,
            '"' => in_double = true,
            ';' => {
                push_command_slice(script, start, idx, &mut command_texts)?;
                operators.push(CompositionOperator::Sequence);
                start = idx + 1;
            }
            '&' => {
                let next = bytes.get(idx + 1).copied().map(char::from);
                if next == Some('&') {
                    push_command_slice(script, start, idx, &mut command_texts)?;
                    operators.push(CompositionOperator::And);
                    start = idx + 2;
                    idx += 1;
                } else {
                    push_construct(&mut unsupported_constructs, "background_operator");
                }
            }
            '|' => {
                let next = bytes.get(idx + 1).copied().map(char::from);
                if next == Some('|') {
                    push_command_slice(script, start, idx, &mut command_texts)?;
                    operators.push(CompositionOperator::Or);
                    start = idx + 2;
                    idx += 1;
                } else if next == Some('&') {
                    push_construct(&mut unsupported_constructs, "pipe_stderr_operator");
                } else {
                    push_command_slice(script, start, idx, &mut command_texts)?;
                    operators.push(CompositionOperator::Pipe);
                    start = idx + 1;
                }
            }
            '>' | '<' => push_construct(&mut unsupported_constructs, "redirection"),
            '`' => push_construct(&mut unsupported_constructs, "substitution_or_expansion"),
            '$' => push_construct(&mut unsupported_constructs, "substitution_or_expansion"),
            '(' | ')' | '{' | '}' => {
                push_construct(&mut unsupported_constructs, "grouping_or_control_flow")
            }
            '\n' | '\r' => push_construct(&mut unsupported_constructs, "newline_separator"),
            '#' => push_construct(&mut unsupported_constructs, "comment"),
            _ => {}
        }

        idx += 1;
    }

    if in_single || in_double || escaped {
        return Err(ShellAnalysisError::Parse(
            "unterminated quote or escape".to_string(),
        ));
    }

    let trailing = script[start..].trim();
    if !trailing.is_empty() {
        command_texts.push(trailing);
    }

    if command_texts.is_empty() {
        return Ok(SplitResult {
            command_texts,
            operators: Vec::new(),
            unsupported_constructs,
        });
    }

    while operators.len() >= command_texts.len() {
        operators.pop();
    }

    if operators.len() + 1 != command_texts.len() {
        return Err(ShellAnalysisError::Parse(
            "invalid command/operator structure".to_string(),
        ));
    }

    Ok(SplitResult {
        command_texts,
        operators,
        unsupported_constructs,
    })
}

fn push_command_slice<'a>(
    script: &'a str,
    start: usize,
    end: usize,
    command_texts: &mut Vec<&'a str>,
) -> Result<(), ShellAnalysisError> {
    let slice = script[start..end].trim();
    if slice.is_empty() {
        return Err(ShellAnalysisError::Parse(
            "missing command between composition operators".to_string(),
        ));
    }
    command_texts.push(slice);
    Ok(())
}

fn tokenize_command(
    command: &str,
    unsupported_constructs: &mut Vec<String>,
) -> Result<Vec<String>, ShellAnalysisError> {
    let mut argv = Vec::new();
    let mut current = String::new();

    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            } else {
                if ch == '$' || ch == '`' {
                    push_construct(unsupported_constructs, "substitution_or_expansion");
                }
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '\'' => in_single = true,
            '"' => in_double = true,
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    argv.push(std::mem::take(&mut current));
                }
            }
            '>' | '<' => {
                push_construct(unsupported_constructs, "redirection");
                current.push(ch);
            }
            '`' | '$' => {
                push_construct(unsupported_constructs, "substitution_or_expansion");
                current.push(ch);
            }
            '(' | ')' | '{' | '}' => {
                push_construct(unsupported_constructs, "grouping_or_control_flow");
                current.push(ch);
            }
            '#' => {
                push_construct(unsupported_constructs, "comment");
                current.push(ch);
            }
            _ => current.push(ch),
        }
    }

    if in_single || in_double || escaped {
        return Err(ShellAnalysisError::Parse(
            "unterminated quote or escape".to_string(),
        ));
    }

    if !current.is_empty() {
        argv.push(current);
    }

    Ok(argv)
}

fn push_construct(unsupported_constructs: &mut Vec<String>, construct: &str) {
    unsupported_constructs.push(construct.to_string());
}

fn dedupe_preserve_order(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::CommandKnowledge;

    #[test]
    fn known_for_supported_composed_commands() {
        let analyzer = BasicShellAnalyzer;
        let analysis = analyzer
            .analyze_shell_lc_script("echo hi && ls -l | wc -l ; pwd || true")
            .expect("parse");

        assert_eq!(analysis.knowledge, CommandKnowledge::Known);
        assert!(analysis.unsupported_constructs.is_empty());
        assert_eq!(analysis.segments.len(), 5);
        assert_eq!(analysis.segments[0].argv, vec!["echo", "hi"]);
        assert_eq!(analysis.segments[0].operator_before, None);
        assert_eq!(analysis.segments[1].argv, vec!["ls", "-l"]);
        assert_eq!(
            analysis.segments[1].operator_before,
            Some(CompositionOperator::And)
        );
        assert_eq!(analysis.segments[2].argv, vec!["wc", "-l"]);
        assert_eq!(
            analysis.segments[2].operator_before,
            Some(CompositionOperator::Pipe)
        );
        assert_eq!(analysis.segments[3].argv, vec!["pwd"]);
        assert_eq!(
            analysis.segments[3].operator_before,
            Some(CompositionOperator::Sequence)
        );
        assert_eq!(analysis.segments[4].argv, vec!["true"]);
        assert_eq!(
            analysis.segments[4].operator_before,
            Some(CompositionOperator::Or)
        );
    }

    #[test]
    fn unsupported_constructs_map_to_uncertain() {
        let analyzer = BasicShellAnalyzer;
        let analysis = analyzer
            .analyze_shell_lc_script("echo hi > out.txt")
            .expect("parse");

        assert_eq!(analysis.knowledge, CommandKnowledge::Uncertain);
        assert!(analysis.segments.is_empty());
        assert!(analysis
            .unsupported_constructs
            .iter()
            .any(|value| value == "redirection"));
    }

    #[test]
    fn malformed_parse_maps_to_error() {
        let analyzer = BasicShellAnalyzer;
        let result = analyzer.analyze_shell_lc_script("echo 'unterminated");

        assert!(matches!(result, Err(ShellAnalysisError::Parse(_))));
    }

    #[test]
    fn supported_syntax_without_segmentable_command_maps_to_unknown() {
        let analyzer = BasicShellAnalyzer;
        let analysis = analyzer.analyze_shell_lc_script(" ").expect("parse");

        assert_eq!(analysis.knowledge, CommandKnowledge::Unknown);
        assert!(analysis.segments.is_empty());
        assert!(analysis.unsupported_constructs.is_empty());
    }
}
