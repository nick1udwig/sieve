# sieve-shell

AST-based shell analysis for `bash -lc` script strings.

## Behavior Matrix

| Condition | Knowledge | Segments | Notes |
| --- | --- | --- | --- |
| Parseable, only supported composition (`;`, `&&`, `||`, `|`), all command argv extractable | `known` | present | ordered `CommandSegment`s with `operator_before` |
| Parseable, no unsupported construct, but no summary-ready command segments | `unknown` | empty | syntax is accepted but command extraction unavailable |
| Unsupported construct found | `uncertain` | empty | `unsupported_constructs` populated |
| Shell syntax error | error | n/a | returns `ShellAnalysisError::Parse` |

## Unsupported Construct Tags

- `redirection`
- `substitution_or_expansion`
- `grouping_or_control_flow`
- `background_operator`
- `pipe_stderr_operator`
- `newline_separator`
- `comment`
