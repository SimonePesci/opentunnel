enum Command {
    Help,
    Version,
    Server { listen_port: u16 },
    Expose,
}

enum ParseError {
    InvalidListenPort,
    MissingListenPort,
    UnknownCommand,
}

pub fn run(args: Vec<String>) -> i32 {
    match parse_command(args.as_slice()) {
        Ok(command) => run_command(command),
        Err(ParseError::MissingListenPort) => {
            eprintln!("error: server requires `--listen <port>`");
            eprintln!("run `opentunnel --help` for usage");
            2
        }
        Err(ParseError::InvalidListenPort) => {
            eprintln!("error: listen port must be a number from 0 to 65535");
            eprintln!("run `opentunnel --help` for usage");
            2
        }
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
        [command] if command == "server" => Err(ParseError::MissingListenPort),
        [command, flag] if command == "server" && flag == "--listen" => {
            Err(ParseError::MissingListenPort)
        }
        [command, flag, port] if command == "server" && flag == "--listen" => {
            parse_port(port).map(|listen_port| Command::Server { listen_port })
        }
        [command] if command == "expose" => Ok(Command::Expose),
        _ => Err(ParseError::UnknownCommand),
    }
}

fn parse_port(value: &str) -> Result<u16, ParseError> {
    value
        .parse::<u16>()
        .map_err(|_| ParseError::InvalidListenPort)
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
        Command::Server { listen_port } => {
            println!("server will listen on port {listen_port}");
            println!("networking is not implemented yet");
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
    println!("  opentunnel server --listen <port>");
    println!("  opentunnel expose");
}
