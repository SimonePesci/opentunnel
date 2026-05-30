use std::net::SocketAddr;

enum Command {
    Help,
    Version,
    Server(ServerConfig),
    Expose(ExposeConfig),
}

struct ServerConfig {
    listen_port: u16,
}

struct ExposeConfig {
    local_port: u16,
    server_address: SocketAddr,
}

enum ParseError {
    InvalidListenPort,
    InvalidLocalPort,
    InvalidServerAddress,
    MissingListenPort,
    MissingLocalPort,
    MissingServerAddress,
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
        Err(ParseError::MissingLocalPort) => {
            eprintln!("error: expose requires `--local <port>`");
            eprintln!("run `opentunnel --help` for usage");
            2
        }
        Err(ParseError::InvalidLocalPort) => {
            eprintln!("error: local port must be a number from 0 to 65535");
            eprintln!("run `opentunnel --help` for usage");
            2
        }
        Err(ParseError::MissingServerAddress) => {
            eprintln!("error: expose requires `--server <address>`");
            eprintln!("run `opentunnel --help` for usage");
            2
        }
        Err(ParseError::InvalidServerAddress) => {
            eprintln!("error: server address must look like 127.0.0.1:8080");
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
            parse_port(port, ParseError::InvalidListenPort)
                .map(|listen_port| Command::Server(ServerConfig { listen_port }))
        }
        [command] if command == "expose" => Err(ParseError::MissingLocalPort),
        [command, flag] if command == "expose" && flag == "--local" => {
            Err(ParseError::MissingLocalPort)
        }
        [command, flag, port] if command == "expose" && flag == "--local" => {
            parse_port(port, ParseError::InvalidLocalPort)?;
            Err(ParseError::MissingServerAddress)
        }
        [command, local_flag, local_port, server_flag]
            if command == "expose" && local_flag == "--local" && server_flag == "--server" =>
        {
            parse_port(local_port, ParseError::InvalidLocalPort)?;
            Err(ParseError::MissingServerAddress)
        }
        [command, local_flag, local_port, server_flag, server_address]
            if command == "expose" && local_flag == "--local" && server_flag == "--server" =>
        {
            let local_port = parse_port(local_port, ParseError::InvalidLocalPort)?;
            let server_address = parse_server_address(server_address)?;

            Ok(Command::Expose(ExposeConfig {
                local_port,
                server_address,
            }))
        }
        _ => Err(ParseError::UnknownCommand),
    }
}

fn parse_port(value: &str, invalid_error: ParseError) -> Result<u16, ParseError> {
    value.parse::<u16>().map_err(|_| invalid_error)
}

fn parse_server_address(value: &str) -> Result<SocketAddr, ParseError> {
    value
        .parse::<SocketAddr>()
        .map_err(|_| ParseError::InvalidServerAddress)
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
        Command::Server(config) => {
            match crate::server::run(config.listen_port) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("error: failed to run server: {error}");
                    1
                }
            }
        }
        Command::Expose(config) => {
            match crate::expose::run(config.local_port, config.server_address) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("error: failed to start expose: {error}");
                    1
                }
            }
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
    println!("  opentunnel expose --local <port> --server <address>");
}
