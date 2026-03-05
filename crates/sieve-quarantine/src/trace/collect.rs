use crate::report::io_err;
use crate::QuarantineRunError;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn collect_trace_files(run_dir: &Path) -> Result<Vec<PathBuf>, QuarantineRunError> {
    let mut files = Vec::new();

    let entries = fs::read_dir(run_dir).map_err(io_err)?;
    for entry in entries {
        let entry = entry.map_err(io_err)?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == "strace" || name.starts_with("strace.") {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}
