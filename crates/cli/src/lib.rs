use clap::Parser;

#[derive(Parser)]
#[command(name = "aurora", version, about = "Aurora command line client")]
struct Cli {}

pub fn run() {
  Cli::parse();
}
