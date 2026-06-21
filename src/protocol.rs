use std::net::SocketAddr;

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
    Ok { tunnel_address: SocketAddr },
    Error(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeResponseParseError {
    Empty,
    InvalidTunnelAddress,
    MissingTunnelAddress,
    MissingErrorMessage,
    UnknownStatus,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ControlMessage {
    IncomingConnection,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ControlMessageParseError {
    Empty,
    UnknownMessage,
}

pub fn expose_handshake(local_port: u16) -> String {
    format!("EXPOSE {local_port}\n")
}

// Includes the bound endpoint so the expose client can show where the tunnel
// is reachable instead of reconstructing server-side binding details.
pub fn ok_response(tunnel_address: SocketAddr) -> String {
    format!("OK {tunnel_address}\n")
}

// Formats a single-line rejection so the expose client can report why the
// server refused registration.
pub fn error_response(message: &str) -> String {
    format!("ERR {message}\n")
}

// Notifies the expose client that the server is holding a tunnel user socket.
pub fn incoming_connection_message() -> &'static str {
    "INCOMING\n"
}

// Parses messages sent after a successful expose handshake.
pub fn parse_control_message(line: &str) -> Result<ControlMessage, ControlMessageParseError> {
    match line.trim_end() {
        "INCOMING" => Ok(ControlMessage::IncomingConnection),
        "" => Err(ControlMessageParseError::Empty),
        _ => Err(ControlMessageParseError::UnknownMessage),
    }
}

// Parses the server's response to the initial expose handshake.
pub fn parse_handshake_response(
    line: &str,
) -> Result<HandshakeResponse<'_>, HandshakeResponseParseError> {
    let line = line.trim_end();

    if let Some(address) = line.strip_prefix("OK ") {
        let tunnel_address = address
            .parse::<SocketAddr>()
            .map_err(|_| HandshakeResponseParseError::InvalidTunnelAddress)?;

        return Ok(HandshakeResponse::Ok { tunnel_address });
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
        "OK" => Err(HandshakeResponseParseError::MissingTunnelAddress),
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
    use super::{
        parse_control_message, parse_handshake_response, ControlMessage, ControlMessageParseError,
        HandshakeResponse, HandshakeResponseParseError,
    };
    use std::net::SocketAddr;

    #[test]
    fn parses_successful_handshake_response() {
        assert_eq!(
            parse_handshake_response("OK 127.0.0.1:3000\n"),
            Ok(HandshakeResponse::Ok {
                tunnel_address: "127.0.0.1:3000"
                    .parse::<SocketAddr>()
                    .expect("test address should parse"),
            })
        );
    }

    #[test]
    fn rejects_success_response_without_tunnel_address() {
        assert_eq!(
            parse_handshake_response("OK\n"),
            Err(HandshakeResponseParseError::MissingTunnelAddress)
        );
    }

    #[test]
    fn rejects_invalid_tunnel_address() {
        assert_eq!(
            parse_handshake_response("OK invalid\n"),
            Err(HandshakeResponseParseError::InvalidTunnelAddress)
        );
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

    #[test]
    fn parses_incoming_connection_message() {
        assert_eq!(super::incoming_connection_message(), "INCOMING\n");
        assert_eq!(
            parse_control_message("INCOMING\n"),
            Ok(ControlMessage::IncomingConnection)
        );
    }

    #[test]
    fn rejects_unknown_control_message() {
        assert_eq!(
            parse_control_message("UNKNOWN\n"),
            Err(ControlMessageParseError::UnknownMessage)
        );
    }
}
