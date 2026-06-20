use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SERVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SERVER_READ_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run(local_port: u16, server_address: SocketAddr) -> io::Result<()> {
    let local_address = SocketAddr::from(([127, 0, 0, 1], local_port));
    let local_stream = TcpStream::connect_timeout(&local_address, LOCAL_CONNECT_TIMEOUT)?;

    println!("connected to local service on {local_address}");
    drop(local_stream);

    let mut server_stream = TcpStream::connect_timeout(&server_address, SERVER_CONNECT_TIMEOUT)?;

    println!("connected to opentunnel server on {server_address}");
    server_stream.write_all(crate::protocol::expose_handshake(local_port).as_bytes())?;
    server_stream.set_read_timeout(Some(SERVER_READ_TIMEOUT))?;

    println!("sent expose handshake for local port {local_port}");
    let mut reader = BufReader::new(server_stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;

    validate_handshake_response(&response)?;

    // The timeout protects only the handshake. Active sessions may otherwise
    // remain idle indefinitely while waiting for incoming tunnel traffic.
    reader.get_mut().set_read_timeout(None)?;

    println!("server accepted expose handshake");
    keep_control_connection(reader)
}

// Keeps the expose process tied to the lifetime of the server-side session.
fn keep_control_connection(reader: BufReader<TcpStream>) -> io::Result<()> {
    println!("expose session active; press Ctrl-C to stop");
    println!("tunneling is not implemented yet");

    monitor_control_connection(reader)
}

// Waits for the first post-handshake control event. The current protocol
// defines no such messages, so either EOF or data means the session is invalid.
fn monitor_control_connection(mut reader: impl BufRead) -> io::Result<()> {
    let mut message = String::new();

    // This read blocks while the session is healthy because the current
    // protocol does not define any messages after the initial OK response.
    if reader.read_line(&mut message)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "opentunnel server closed the control connection",
        ));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected server control message: {}", message.trim_end()),
    ))
}

// Converts the wire response into errors meaningful to the CLI caller.
fn validate_handshake_response(response: &str) -> io::Result<()> {
    match crate::protocol::parse_handshake_response(response) {
        Ok(crate::protocol::HandshakeResponse::Ok) => Ok(()),
        Ok(crate::protocol::HandshakeResponse::Error(message)) => Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("opentunnel server rejected expose: {message}"),
        )),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid server handshake response: {}", response.trim_end()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{monitor_control_connection, validate_handshake_response};
    use std::io::{self, Cursor};

    #[test]
    fn rejected_handshake_returns_server_reason() {
        let error = validate_handshake_response("ERR tunnel port unavailable\n")
            .expect_err("server rejection should fail");

        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
        assert!(error.to_string().contains("tunnel port unavailable"));
    }

    #[test]
    fn malformed_handshake_response_is_rejected() {
        let error =
            validate_handshake_response("WAIT\n").expect_err("malformed response should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn closed_control_connection_returns_connection_error() {
        let error = monitor_control_connection(Cursor::new(Vec::<u8>::new()))
            .expect_err("closed control connection should fail");

        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }

    #[test]
    fn unexpected_control_message_is_rejected() {
        let error = monitor_control_connection(Cursor::new(b"UNKNOWN\n"))
            .expect_err("unexpected control message should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
