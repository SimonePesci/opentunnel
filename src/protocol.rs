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

#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeResponse<'a> {
    Ok,
    Error(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeResponseParseError {
    Empty,
    MissingErrorMessage,
    UnknownStatus,
}

pub fn expose_handshake(local_port: u16) -> String {
    format!("EXPOSE {local_port}\n")
}

pub fn ok_response() -> &'static str {
    "OK\n"
}

// Formats a single-line rejection so the expose client can report why the
// server refused registration.
pub fn error_response(message: &str) -> String {
    format!("ERR {message}\n")
}

// Parses the server's response to the initial expose handshake.
pub fn parse_handshake_response(
    line: &str,
) -> Result<HandshakeResponse<'_>, HandshakeResponseParseError> {
    let line = line.trim_end();

    if line == "OK" {
        return Ok(HandshakeResponse::Ok);
    }

    if let Some(message) = line.strip_prefix("ERR ") {
        return if message.is_empty() {
            Err(HandshakeResponseParseError::MissingErrorMessage)
        } else {
            Ok(HandshakeResponse::Error(message))
        };
    }

    match line {
        "" => Err(HandshakeResponseParseError::Empty),
        "ERR" => Err(HandshakeResponseParseError::MissingErrorMessage),
        _ => Err(HandshakeResponseParseError::UnknownStatus),
    }
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

#[cfg(test)]
mod tests {
    use super::{parse_handshake_response, HandshakeResponse, HandshakeResponseParseError};

    #[test]
    fn parses_successful_handshake_response() {
        assert_eq!(parse_handshake_response("OK\n"), Ok(HandshakeResponse::Ok));
    }

    #[test]
    fn parses_handshake_error_message() {
        assert_eq!(
            parse_handshake_response("ERR tunnel port unavailable\n"),
            Ok(HandshakeResponse::Error("tunnel port unavailable"))
        );
    }

    #[test]
    fn rejects_handshake_error_without_message() {
        assert_eq!(
            parse_handshake_response("ERR\n"),
            Err(HandshakeResponseParseError::MissingErrorMessage)
        );
    }

    #[test]
    fn rejects_unknown_handshake_response_status() {
        assert_eq!(
            parse_handshake_response("WAIT\n"),
            Err(HandshakeResponseParseError::UnknownStatus)
        );
    }
}
