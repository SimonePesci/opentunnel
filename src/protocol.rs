pub enum Handshake {
    Expose { local_port: u16 },
}

pub enum HandshakeParseError {
    Empty,
    ExtraParts,
    InvalidPort,
    MissingPort,
    UnknownCommand,
}

pub fn expose_handshake(local_port: u16) -> String {
    format!("EXPOSE {local_port}\n")
}

pub fn ok_response() -> &'static str {
    "OK\n"
}

pub fn error_response() -> &'static str {
    "ERR\n"
}

pub fn is_ok_response(line: &str) -> bool {
    line.trim_end() == "OK"
}

pub fn parse_handshake(line: &str) -> Result<Handshake, HandshakeParseError> {
    let mut parts = line.split_whitespace();

    match (parts.next(), parts.next(), parts.next()) {
        (Some("EXPOSE"), Some(port), None) => {
            let local_port = port
                .parse::<u16>()
                .map_err(|_| HandshakeParseError::InvalidPort)?;

            Ok(Handshake::Expose { local_port })
        }
        (Some("EXPOSE"), None, None) => Err(HandshakeParseError::MissingPort),
        (Some("EXPOSE"), Some(_), Some(_)) => Err(HandshakeParseError::ExtraParts),
        (Some(_), _, _) => Err(HandshakeParseError::UnknownCommand),
        (None, _, _) => Err(HandshakeParseError::Empty),
    }
}

pub fn describe_parse_error(error: HandshakeParseError) -> &'static str {
    match error {
        HandshakeParseError::Empty => "empty handshake",
        HandshakeParseError::ExtraParts => "too many handshake parts",
        HandshakeParseError::InvalidPort => "invalid expose port",
        HandshakeParseError::MissingPort => "missing expose port",
        HandshakeParseError::UnknownCommand => "unknown handshake command",
    }
}
