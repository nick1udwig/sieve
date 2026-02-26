#![forbid(unsafe_code)]

use sieve_types::{CommandKnowledge, CommandSegment, CompositionOperator};
use thiserror::Error;
use tree_sitter::{Node, Parser, Tree};
use tree_sitter_bash::LANGUAGE as BASH;

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
        if script.trim().is_empty() {
            return Ok(ShellAnalysis {
                knowledge: CommandKnowledge::Unknown,
                segments: Vec::new(),
                unsupported_constructs: Vec::new(),
            });
        }

        let tree = parse_shell(script)?;
        let root = tree.root_node();
        if root.has_error() {
            return Err(ShellAnalysisError::Parse("shell syntax error".to_string()));
        }

        let unsupported = collect_unsupported_constructs(root, script);
        if !unsupported.is_empty() {
            return Ok(ShellAnalysis {
                knowledge: CommandKnowledge::Uncertain,
                segments: Vec::new(),
                unsupported_constructs: unsupported,
            });
        }

        let extraction = extract_segments(root, script);
        if !extraction.unsupported_constructs.is_empty() {
            return Ok(ShellAnalysis {
                knowledge: CommandKnowledge::Uncertain,
                segments: Vec::new(),
                unsupported_constructs: dedupe_preserve_order(extraction.unsupported_constructs),
            });
        }

        if let Some(segments) = extraction.segments {
            return Ok(ShellAnalysis {
                knowledge: CommandKnowledge::Known,
                segments,
                unsupported_constructs: Vec::new(),
            });
        }

        Ok(ShellAnalysis {
            knowledge: CommandKnowledge::Unknown,
            segments: Vec::new(),
            unsupported_constructs: Vec::new(),
        })
    }
}

fn parse_shell(script: &str) -> Result<Tree, ShellAnalysisError> {
    let mut parser = Parser::new();
    let language = BASH.into();
    parser.set_language(&language).map_err(|error| {
        ShellAnalysisError::Parse(format!("failed to load bash grammar: {error}"))
    })?;
    parser
        .parse(script, None)
        .ok_or_else(|| ShellAnalysisError::Parse("tree-sitter returned no parse tree".to_string()))
}

fn collect_unsupported_constructs(root: Node<'_>, script: &str) -> Vec<String> {
    let mut unsupported = Vec::new();
    if script.contains('\n') || script.contains('\r') {
        unsupported.push("newline_separator".to_string());
    }

    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(kind) = classify_unsupported_construct(node.kind()) {
            unsupported.push(kind.to_string());
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    dedupe_preserve_order(unsupported)
}

fn classify_unsupported_construct(kind: &str) -> Option<&'static str> {
    match kind {
        "file_redirect"
        | "heredoc_redirect"
        | "herestring_redirect"
        | "redirected_statement"
        | "heredoc_body"
        | "simple_heredoc_body"
        | "heredoc_content"
        | "heredoc_start"
        | "heredoc_end" => Some("redirection"),
        "command_substitution"
        | "process_substitution"
        | "arithmetic_expansion"
        | "expansion"
        | "simple_expansion"
        | "brace_expression"
        | "translated_string"
        | "ansi_c_string" => Some("substitution_or_expansion"),
        "compound_statement"
        | "subshell"
        | "if_statement"
        | "for_statement"
        | "c_style_for_statement"
        | "while_statement"
        | "case_statement"
        | "case_item"
        | "function_definition"
        | "do_group"
        | "elif_clause"
        | "else_clause"
        | "negated_command"
        | "test_command"
        | "parenthesized_expression" => Some("grouping_or_control_flow"),
        "comment" => Some("comment"),
        "&" => Some("background_operator"),
        "|&" => Some("pipe_stderr_operator"),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct SegmentExtraction {
    segments: Option<Vec<CommandSegment>>,
    unsupported_constructs: Vec<String>,
}

fn extract_segments(root: Node<'_>, script: &str) -> SegmentExtraction {
    let mut command_nodes = collect_command_nodes(root);
    if command_nodes.is_empty() {
        return SegmentExtraction {
            segments: None,
            unsupported_constructs: Vec::new(),
        };
    }

    command_nodes.sort_by_key(|node| node.start_byte());
    let mut segments = Vec::with_capacity(command_nodes.len());
    let mut unsupported_constructs = Vec::new();
    let mut previous_end = 0usize;

    for (index, command_node) in command_nodes.into_iter().enumerate() {
        let argv = match parse_plain_command_from_node(command_node, script) {
            Some(argv) if !argv.is_empty() => argv,
            _ => {
                return SegmentExtraction {
                    segments: None,
                    unsupported_constructs,
                };
            }
        };

        let operator_before = if index == 0 {
            None
        } else {
            let between = &script[previous_end..command_node.start_byte()];
            match classify_operator_between(between) {
                BetweenOperator::Supported(operator) => Some(operator),
                BetweenOperator::Unsupported(kind) => {
                    unsupported_constructs.push(kind.to_string());
                    None
                }
                BetweenOperator::Unknown => {
                    return SegmentExtraction {
                        segments: None,
                        unsupported_constructs,
                    };
                }
            }
        };

        segments.push(CommandSegment {
            argv,
            operator_before,
        });
        previous_end = command_node.end_byte();
    }

    if unsupported_constructs.is_empty() {
        SegmentExtraction {
            segments: Some(segments),
            unsupported_constructs,
        }
    } else {
        SegmentExtraction {
            segments: None,
            unsupported_constructs: dedupe_preserve_order(unsupported_constructs),
        }
    }
}

fn collect_command_nodes(root: Node<'_>) -> Vec<Node<'_>> {
    let mut command_nodes = Vec::new();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        if node.kind() == "command" {
            command_nodes.push(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    command_nodes
}

fn parse_plain_command_from_node(command_node: Node<'_>, source: &str) -> Option<Vec<String>> {
    if command_node.kind() != "command" {
        return None;
    }

    let mut argv = Vec::new();
    let mut cursor = command_node.walk();

    for child in command_node.named_children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                let word_node = child.named_child(0)?;
                argv.push(parse_word_atom(word_node, source)?);
            }
            "word" | "number" | "string" | "raw_string" | "concatenation" => {
                argv.push(parse_word_atom(child, source)?);
            }
            "comment" => {}
            _ => return None,
        }
    }

    if argv.is_empty() {
        None
    } else {
        Some(argv)
    }
}

fn parse_word_atom(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "word" | "number" => parse_word_or_number(node, source),
        "string" => parse_double_quoted_string(node, source),
        "raw_string" => parse_raw_string(node, source),
        "concatenation" => parse_concatenation(node, source),
        _ => None,
    }
}

fn parse_word_or_number(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    if node.named_children(&mut cursor).next().is_some() {
        return None;
    }
    node.utf8_text(source.as_bytes()).ok().map(str::to_string)
}

fn parse_double_quoted_string(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        if part.kind() != "string_content" {
            return None;
        }
    }

    let raw = node.utf8_text(source.as_bytes()).ok()?;
    strip_wrapping(raw, '"').map(str::to_string)
}

fn parse_raw_string(node: Node<'_>, source: &str) -> Option<String> {
    let raw = node.utf8_text(source.as_bytes()).ok()?;
    strip_wrapping(raw, '\'').map(str::to_string)
}

fn parse_concatenation(node: Node<'_>, source: &str) -> Option<String> {
    let mut value = String::new();
    let mut cursor = node.walk();

    for part in node.named_children(&mut cursor) {
        value.push_str(&parse_word_atom(part, source)?);
    }

    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn strip_wrapping(input: &str, quote: char) -> Option<&str> {
    input
        .strip_prefix(quote)
        .and_then(|text| text.strip_suffix(quote))
}

enum BetweenOperator {
    Supported(CompositionOperator),
    Unsupported(&'static str),
    Unknown,
}

fn classify_operator_between(text: &str) -> BetweenOperator {
    if text.chars().any(|ch| ch == '\n' || ch == '\r') {
        return BetweenOperator::Unsupported("newline_separator");
    }

    let compact: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    match compact.as_str() {
        ";" => BetweenOperator::Supported(CompositionOperator::Sequence),
        "&&" => BetweenOperator::Supported(CompositionOperator::And),
        "||" => BetweenOperator::Supported(CompositionOperator::Or),
        "|" => BetweenOperator::Supported(CompositionOperator::Pipe),
        "&" => BetweenOperator::Unsupported("background_operator"),
        "|&" => BetweenOperator::Unsupported("pipe_stderr_operator"),
        "" => BetweenOperator::Unknown,
        _ if compact.contains('<') || compact.contains('>') => {
            BetweenOperator::Unsupported("redirection")
        }
        _ => BetweenOperator::Unknown,
    }
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
mod tests;
