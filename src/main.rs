mod cli;
mod cmd;
mod vm;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("✗ {e:#}");
        std::process::exit(1);
    }
}
