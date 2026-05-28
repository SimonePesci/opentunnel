enum Command {
    Help,
    Version,
    Server,
    Expose,
}

enum ParseError {
    UnknownCommand,
}

pub fn run(args: Vec<String>) -> i32 {
    match parse_command(args.as_slice()) {
        Ok(command) => run_command(command),
        Err(ParseError::UnknownCommand) => {
            eprintln!("error: unknown command");
            eprintln!("run `opentunnel --help` for usage");
            2
        }
    }
}

fn parse_command(args: &[String]) -> Result<Command, ParseError> {
    match args {
        [] => Ok(Command::Help),
        [arg] if arg == "--help" || arg == "-h" => Ok(Command::Help),
        [arg] if arg == "--version" || arg == "-V" => Ok(Command::Version),
        [command] if command == "server" => Ok(Command::Server),
        [command] if command == "expose" => Ok(Command::Expose),
        _ => Err(ParseError::UnknownCommand),
    }
}

fn run_command(command: Command) -> i32 {
    match command {
        Command::Help => {
            print_help();
            0
        }
        Command::Version => {
            println!("opentunnel {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Server => {
            println!("server mode is not implemented yet");
            0
        }
        Command::Expose => {
            println!("expose mode is not implemented yet");
            0
        }
    }
}

fn print_help() {
    println!("OpenTunnel");
    println!();
    println!("Usage:");
    println!("  opentunnel --help");
    println!("  opentunnel --version");
    println!("  opentunnel server");
    println!("  opentunnel expose");
}
