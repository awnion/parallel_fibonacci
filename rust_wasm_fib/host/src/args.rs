use anyhow::{Result, anyhow, bail};
use std::env;
use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::PathBuf;

const DEFAULT_RUNTIME_IMPORT: &str = "rust-wasm-runtime:process/runtime";
const DEFAULT_SUPERVISOR_IMPORT: &str = "rust-wasm-runtime:supervisor/runtime";
const DEFAULT_SUPERVISOR_IMPORT_FUNCTION: &str = "spawn-link";
const NO_INIT: &str = "-";

pub(crate) struct RunArgs {
    pub(crate) component_path: PathBuf,
    pub(crate) function: String,
    pub(crate) arg: u64,
    pub(crate) workers: usize,
    pub(crate) runtime_import: String,
}

#[derive(Clone)]
pub(crate) struct ChildInit {
    pub(crate) function: String,
    pub(crate) arg: u64,
}

pub(crate) struct SpawnLinkArgs {
    pub(crate) supervisor_path: PathBuf,
    pub(crate) supervisor_function: String,
    pub(crate) child_path: PathBuf,
    pub(crate) child_function: String,
    pub(crate) arg: u64,
    pub(crate) retries: u32,
    pub(crate) child_init: Option<ChildInit>,
    pub(crate) runtime_import: String,
    pub(crate) runtime_function: String,
}

pub(crate) enum Command {
    Run(RunArgs),
    SpawnLink(SpawnLinkArgs),
}

pub(crate) fn parse_command() -> Result<Command> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run-u64") => parse_run_command(&args[1..]),
        Some("spawn-link-u64") | Some("supervise-u64") => parse_spawn_link_command(&args[1..]),
        _ => bail!(
            "usage:\n  run-u64 <component> <function> <arg> [workers] [runtime-import]\n  spawn-link-u64 <supervisor> <supervisor-function> <child> <child-function> <arg> <retries> <child-init-function|-> <child-init-arg> [runtime-import] [runtime-function]"
        ),
    }
}

fn parse_run_command(args: &[String]) -> Result<Command> {
    let component_path = arg_path(args, 0, "component")?;
    let function = arg_string(args, 1, "function")?;
    let arg = arg_parse(args, 2, "arg")?;
    let workers = args
        .get(3)
        .map(|value| parse_named(value, "workers"))
        .transpose()?
        .filter(|workers| *workers > 0)
        .unwrap_or_else(default_threads);
    let runtime_import = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| DEFAULT_RUNTIME_IMPORT.to_string());

    Ok(Command::Run(RunArgs {
        component_path,
        function,
        arg,
        workers,
        runtime_import,
    }))
}

fn parse_spawn_link_command(args: &[String]) -> Result<Command> {
    let supervisor_path = arg_path(args, 0, "supervisor")?;
    let supervisor_function = arg_string(args, 1, "supervisor-function")?;
    let child_path = arg_path(args, 2, "child")?;
    let child_function = arg_string(args, 3, "child-function")?;
    let arg = arg_parse(args, 4, "arg")?;
    let retries = arg_parse(args, 5, "retries")?;
    let init_function = arg_string(args, 6, "child-init-function")?;
    let init_arg = arg_parse(args, 7, "child-init-arg")?;
    let runtime_import = args
        .get(8)
        .cloned()
        .unwrap_or_else(|| DEFAULT_SUPERVISOR_IMPORT.to_string());
    let runtime_function = args
        .get(9)
        .cloned()
        .unwrap_or_else(|| DEFAULT_SUPERVISOR_IMPORT_FUNCTION.to_string());
    let child_init = (init_function != NO_INIT).then_some(ChildInit {
        function: init_function,
        arg: init_arg,
    });

    Ok(Command::SpawnLink(SpawnLinkArgs {
        supervisor_path,
        supervisor_function,
        child_path,
        child_function,
        arg,
        retries,
        child_init,
        runtime_import,
        runtime_function,
    }))
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

fn arg_string(args: &[String], index: usize, name: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| anyhow!("missing argument: {name}"))
}

fn arg_path(args: &[String], index: usize, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(arg_string(args, index, name)?))
}

fn arg_parse<T>(args: &[String], index: usize, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: Display,
{
    parse_named(
        args.get(index)
            .ok_or_else(|| anyhow!("missing argument: {name}"))?,
        name,
    )
}

fn parse_named<T>(value: &str, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: Display,
{
    value
        .parse()
        .map_err(|error| anyhow!("invalid {name} value {value:?}: {error}"))
}
