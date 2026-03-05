use sieve_captrace::CapTraceError;
use sieve_quarantine::QuarantineNetworkMode;
use std::env;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct CliArgs {
    pub(crate) command: String,
    pub(crate) seed_shell_cases: Vec<String>,
    pub(crate) include_llm_cases: bool,
    pub(crate) max_llm_cases: usize,
    pub(crate) output_path: Option<PathBuf>,
    pub(crate) rust_output_path: Option<PathBuf>,
    pub(crate) logs_root: PathBuf,
    pub(crate) network_mode: CliNetworkMode,
    pub(crate) allow_write_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CliNetworkMode {
    Isolated,
    LocalOnly,
    Full,
}

impl CliNetworkMode {
    pub(crate) fn as_quarantine_mode(self) -> QuarantineNetworkMode {
        match self {
            CliNetworkMode::Isolated => QuarantineNetworkMode::Isolated,
            CliNetworkMode::LocalOnly => QuarantineNetworkMode::LocalOnly,
            CliNetworkMode::Full => QuarantineNetworkMode::Full,
        }
    }
}

pub(crate) fn load_dotenv_if_present() -> Result<(), CapTraceError> {
    match dotenvy::from_filename(".env") {
        Ok(_) => Ok(()),
        Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CapTraceError::Io(format!("failed to load .env: {err}"))),
    }
}

pub(crate) fn parse_cli_args<I>(iter: I) -> Result<CliArgs, CapTraceError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = iter.into_iter();
    let mut command: Option<String> = None;
    let mut seed_shell_cases = Vec::new();
    let mut include_llm_cases = true;
    let mut max_llm_cases = 4usize;
    let mut output_path = None;
    let mut rust_output_path = None;
    let mut logs_root = env::temp_dir().join("sieve-captrace-traces");
    let mut network_mode = CliNetworkMode::Isolated;
    let mut allow_write_paths = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--seed-case" => {
                let value = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --seed-case".to_string())
                })?;
                seed_shell_cases.push(value);
            }
            "--no-llm" => include_llm_cases = false,
            "--max-llm-cases" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --max-llm-cases".to_string())
                })?;
                max_llm_cases = raw.parse::<usize>().map_err(|err| {
                    CapTraceError::Args(format!("invalid --max-llm-cases `{raw}`: {err}"))
                })?;
            }
            "--output" => {
                let raw = args
                    .next()
                    .ok_or_else(|| CapTraceError::Args("missing value for --output".to_string()))?;
                output_path = Some(PathBuf::from(raw));
            }
            "--rust-output" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --rust-output".to_string())
                })?;
                rust_output_path = Some(PathBuf::from(raw));
            }
            "--logs-root" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --logs-root".to_string())
                })?;
                logs_root = PathBuf::from(raw);
            }
            "--allow-local-network" => {
                if network_mode == CliNetworkMode::Full {
                    return Err(CapTraceError::Args(
                        "cannot combine --allow-local-network with --allow-full-network"
                            .to_string(),
                    ));
                }
                network_mode = CliNetworkMode::LocalOnly;
            }
            "--allow-full-network" => {
                if network_mode == CliNetworkMode::LocalOnly {
                    return Err(CapTraceError::Args(
                        "cannot combine --allow-local-network with --allow-full-network"
                            .to_string(),
                    ));
                }
                network_mode = CliNetworkMode::Full;
            }
            "--allow-write" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --allow-write".to_string())
                })?;
                allow_write_paths.push(parse_allow_write_path(&raw)?);
            }
            value if value.starts_with("--") => {
                return Err(CapTraceError::Args(format!("unknown flag `{value}`")));
            }
            value => {
                if command.is_some() {
                    return Err(CapTraceError::Args(format!(
                        "unexpected positional arg `{value}`"
                    )));
                }
                command = Some(value.to_string());
            }
        }
    }

    let command = command
        .ok_or_else(|| CapTraceError::Args("missing required positional <command>".to_string()))?;
    Ok(CliArgs {
        command,
        seed_shell_cases,
        include_llm_cases,
        max_llm_cases,
        output_path,
        rust_output_path,
        logs_root,
        network_mode,
        allow_write_paths,
    })
}

fn parse_allow_write_path(raw: &str) -> Result<PathBuf, CapTraceError> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(CapTraceError::Args(format!(
            "--allow-write path must be absolute: `{raw}`"
        )));
    }
    if !path.exists() {
        return Err(CapTraceError::Args(format!(
            "--allow-write path does not exist: `{raw}`"
        )));
    }
    Ok(path)
}

fn print_usage() {
    let usage = "\
Usage:
  sieve-captrace <command> [--seed-case '<shell cmd>']... [--no-llm]
                [--max-llm-cases <N>] [--output <path>]
                [--rust-output <path>] [--logs-root <path>]
                [--allow-local-network|--allow-full-network]
                [--allow-write <absolute path>]...

Examples:
  sieve-captrace mkdir --seed-case 'mkdir -p {{TMP_DIR}}/logs' --rust-output /tmp/mkdir.rs
  sieve-captrace cp --output /tmp/cp-definition.json
  sieve-captrace curl --allow-full-network --allow-write /root/.cache --output /tmp/curl-definition.json
";
    println!("{usage}");
}
