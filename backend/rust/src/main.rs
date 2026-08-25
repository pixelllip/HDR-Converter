use clap::Parser;

fn main() {
    let cli = hdrconv::cli::Cli::parse();
    if let Err(e) = hdrconv::run(cli) {
        eprintln!("错误: {e:#}");
        std::process::exit(1);
    }
}