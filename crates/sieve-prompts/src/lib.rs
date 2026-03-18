pub mod codex {
    pub const IMAGE_OCR: &str = include_str!("codex/image_ocr.md");
}

pub mod guidance {
    pub const INSTRUCTION: &str = include_str!("guidance/instruction.md");
    pub const SYSTEM: &str = include_str!("guidance/system.md");
}

pub mod heartbeat {
    pub const EVENTS: &str = include_str!("heartbeat/events.md");
    pub const IDLE: &str = include_str!("heartbeat/idle.md");
}

pub mod planner {
    pub const REGENERATION_DIAGNOSTIC: &str = include_str!("planner/regeneration_diagnostic.md");
    pub const SYSTEM: &str = include_str!("planner/system.md");
}

pub mod response {
    pub const SYSTEM: &str = include_str!("response/system.md");
}

pub mod summary {
    pub const SYSTEM: &str = include_str!("summary/system.md");
}
