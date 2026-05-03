mod args;
mod codec;
mod component_runtime;
mod engine;
mod status;
mod supervisor;
mod util;

use anyhow::Result;
use args::Command;
use tokio::runtime::Builder;

fn main() -> Result<()> {
    match args::parse_command()? {
        Command::Run(args) => {
            let runtime = Builder::new_multi_thread()
                .worker_threads(args.workers)
                .enable_all()
                .build()?;
            let result = runtime.block_on(component_runtime::run_component(&args))?;
            println!("{result}");
            Ok(())
        }
        Command::SpawnLink(args) => supervisor::run_spawn_link(&args),
    }
}
