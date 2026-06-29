use crate::response_style::dedupe_preserve_order;

pub(crate) fn extract_trusted_evidence_lines(
    trusted_user_message: &str,
    planner_thoughts: Option<&str>,
) -> Vec<String> {
    let mut lines = vec![format!("[user] {trusted_user_message}")];
    if let Some(thoughts) = planner_thoughts {
        for line in thoughts.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("[user] ") {
                lines.push(trimmed.to_string());
            }
        }
    }
    dedupe_preserve_order(lines)
}

pub(crate) fn combine_gate_reasons(gates: &[Option<String>]) -> Option<String> {
    let mut combined = Vec::new();
    for gate in gates {
        if let Some(gate) = gate.as_deref() {
            let trimmed = gate.trim();
            if !trimmed.is_empty() {
                combined.push(trimmed.to_string());
            }
        }
    }
    if combined.is_empty() {
        None
    } else {
        Some(combined.join(" | "))
    }
}
