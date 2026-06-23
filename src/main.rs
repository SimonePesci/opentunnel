mod cli;
mod expose;
mod protocol;
mod relay;
mod server;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let exit_code = cli::run(args);

    std::process::exit(exit_code);
}
