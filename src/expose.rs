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

    let (tunnel_address, session_id) = validate_handshake_response(&response)?;

    // The timeout protects only the handshake. Active sessions may otherwise
    // remain idle indefinitely while waiting for incoming tunnel traffic.
    reader.get_mut().set_read_timeout(None)?;

    println!("server accepted expose handshake");
    println!("registered expose session {session_id}");
    println!("tunnel available on {tunnel_address}");
    keep_control_connection(reader, server_address, session_id)
}

// Keeps the expose process tied to the lifetime of the server-side session.
fn keep_control_connection(
    reader: BufReader<TcpStream>,
    server_address: SocketAddr,
    session_id: u64,
) -> io::Result<()> {
    println!("expose session active; press Ctrl-C to stop");
    println!("tunneling is not implemented yet");

    monitor_control_connection(reader, || {
        open_forward_connection(server_address, session_id)
    })
}

// Processes post-handshake server events until the control connection closes.
fn monitor_control_connection<R, F, T>(mut reader: R, mut open_forward: F) -> io::Result<()>
where
    R: BufRead,
    F: FnMut() -> io::Result<T>,
{
    let mut message = String::new();
    let mut forward_connection = None;

    loop {
        message.clear();

        if reader.read_line(&mut message)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "opentunnel server closed the control connection",
            ));
        }

        match crate::protocol::parse_control_message(&message) {
            Ok(crate::protocol::ControlMessage::IncomingConnection) => {
                if forward_connection.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "forward connection already registered",
                    ));
                }

                println!("incoming tunnel connection is waiting on the server");
                forward_connection = Some(open_forward()?);
                println!("forward connection registered with server");
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected server control message: {}", message.trim_end()),
                ));
            }
        }
    }
}

// Opens the raw data channel separately from the line-oriented control stream.
fn open_forward_connection(server_address: SocketAddr, session_id: u64) -> io::Result<TcpStream> {
    let mut stream = TcpStream::connect_timeout(&server_address, SERVER_CONNECT_TIMEOUT)?;
    stream.write_all(crate::protocol::forward_handshake(session_id).as_bytes())?;
    stream.set_read_timeout(Some(SERVER_READ_TIMEOUT))?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    validate_forward_response(&response)?;

    let stream = reader.into_inner();
    stream.set_read_timeout(None)?;
    Ok(stream)
}

fn validate_forward_response(response: &str) -> io::Result<()> {
    if crate::protocol::is_forward_ready_response(response) {
        return Ok(());
    }

    if let Some(message) = response.trim_end().strip_prefix("ERR ") {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("opentunnel server rejected forward connection: {message}"),
        ));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid server forward response: {}", response.trim_end()),
    ))
}

// Converts the wire response into errors meaningful to the CLI caller.
fn validate_handshake_response(response: &str) -> io::Result<(SocketAddr, u64)> {
    match crate::protocol::parse_handshake_response(response) {
        Ok(crate::protocol::HandshakeResponse::Ok {
            tunnel_address,
            session_id,
        }) => Ok((tunnel_address, session_id)),
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
    use super::{
        monitor_control_connection, validate_forward_response, validate_handshake_response,
    };
    use std::cell::Cell;
    use std::io::{self, Cursor};
    use std::net::SocketAddr;

    #[test]
    fn accepted_handshake_returns_tunnel_address() {
        let (tunnel_address, session_id) = validate_handshake_response("OK 127.0.0.1:3000 42\n")
            .expect("successful response should return tunnel address");

        assert_eq!(
            tunnel_address,
            "127.0.0.1:3000"
                .parse::<SocketAddr>()
                .expect("test address should parse")
        );
        assert_eq!(session_id, 42);
    }

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
        let error = monitor_control_connection(Cursor::new(Vec::<u8>::new()), || Ok(()))
            .expect_err("closed control connection should fail");

        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }

    #[test]
    fn incoming_connection_message_keeps_monitoring() {
        let forward_calls = Cell::new(0);
        let error = monitor_control_connection(Cursor::new(b"INCOMING\n"), || {
            forward_calls.set(forward_calls.get() + 1);
            Ok(())
        })
        .expect_err("EOF after a valid message should close the session");

        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(forward_calls.get(), 1);
    }

    #[test]
    fn unexpected_control_message_is_rejected() {
        let error = monitor_control_connection(Cursor::new(b"UNKNOWN\n"), || Ok(()))
            .expect_err("unexpected control message should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn validates_forward_ready_and_error_responses() {
        assert!(validate_forward_response("READY\n").is_ok());

        let error = validate_forward_response("ERR unknown expose session\n")
            .expect_err("forward rejection should fail");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
    }
}
