use crate::{BasicShellAnalyzer, ShellAnalysisError, ShellAnalyzer};
use sieve_types::{CommandKnowledge, CompositionOperator};

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

#[test]
fn control_flow_constructs_map_to_uncertain() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("if true; then echo hi; fi")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Uncertain);
    assert!(analysis.segments.is_empty());
    assert!(analysis
        .unsupported_constructs
        .iter()
        .any(|value| value == "grouping_or_control_flow"));
}

#[test]
fn literal_hash_in_word_stays_known() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("echo foo#bar")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Known);
    assert!(analysis.unsupported_constructs.is_empty());
    assert_eq!(analysis.segments.len(), 1);
    assert_eq!(analysis.segments[0].argv, vec!["echo", "foo#bar"]);
    assert_eq!(analysis.segments[0].operator_before, None);
}

#[test]
fn parseable_but_non_extractable_shape_maps_to_unknown() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("declare foo=bar")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Unknown);
    assert!(analysis.segments.is_empty());
    assert!(analysis.unsupported_constructs.is_empty());
}

#[test]
fn background_operator_maps_to_uncertain() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("sleep 1 &")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Uncertain);
    assert!(analysis.segments.is_empty());
    assert!(analysis
        .unsupported_constructs
        .iter()
        .any(|value| value == "background_operator"));
}

#[test]
fn expansions_map_to_uncertain() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("echo $HOME")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Uncertain);
    assert!(analysis.segments.is_empty());
    assert!(analysis
        .unsupported_constructs
        .iter()
        .any(|value| value == "substitution_or_expansion"));
}

#[test]
fn heredoc_maps_to_uncertain() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("cat <<EOF\nhello\nEOF")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Uncertain);
    assert!(analysis.segments.is_empty());
    assert!(analysis
        .unsupported_constructs
        .iter()
        .any(|value| value == "redirection"));
}

#[test]
fn process_substitution_maps_to_uncertain() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("diff <(ls) <(pwd)")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Uncertain);
    assert!(analysis.segments.is_empty());
    assert!(analysis
        .unsupported_constructs
        .iter()
        .any(|value| value == "substitution_or_expansion"));
}

#[test]
fn pipe_stderr_operator_maps_to_uncertain() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script("echo hi |& wc -c")
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Uncertain);
    assert!(analysis.segments.is_empty());
    assert!(analysis
        .unsupported_constructs
        .iter()
        .any(|value| value == "pipe_stderr_operator"));
}

#[test]
fn concatenated_quotes_extract_as_known_segment() {
    let analyzer = BasicShellAnalyzer;
    let analysis = analyzer
        .analyze_shell_lc_script(r#"echo "/usr"'/'"local"/bin"#)
        .expect("parse");

    assert_eq!(analysis.knowledge, CommandKnowledge::Known);
    assert!(analysis.unsupported_constructs.is_empty());
    assert_eq!(analysis.segments.len(), 1);
    assert_eq!(analysis.segments[0].argv, vec!["echo", "/usr/local/bin"]);
}
