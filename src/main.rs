use clap::Parser;
use wasmedgeup::cli::Cli;

fn main() {
    let cli = Cli::parse();
    println!("{:?}", cli);
}
