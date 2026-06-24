use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
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
    keep_control_connection(reader, server_address, session_id, local_address)
}

// Keeps the expose process tied to the lifetime of the server-side session.
fn keep_control_connection(
    reader: BufReader<TcpStream>,
    server_address: SocketAddr,
    session_id: u64,
    local_address: SocketAddr,
) -> io::Result<()> {
    println!("expose session active; press Ctrl-C to stop");

    monitor_control_connection(reader, || {
        start_forwarding(server_address, session_id, local_address)
    })
}

// Processes post-handshake server events until the control connection closes.
fn monitor_control_connection<R, F, T>(mut reader: R, mut open_forward: F) -> io::Result<()>
where
    R: BufRead,
    F: FnMut() -> io::Result<T>,
{
    let mut message = String::new();

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
                println!("incoming tunnel connection is waiting on the server");
                // Each INCOMING notification represents one waiting tunnel
                // user, so the expose client opens one matching data channel.
                // Dropping the JoinHandle is fine: the relay thread continues
                // independently while the control connection listens for more.
                let _forward_connection = open_forward()?;
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

    let response = read_forward_response(&mut stream)?;
    validate_forward_response(&response)?;

    stream.set_read_timeout(None)?;
    Ok(stream)
}

// Reads only through the protocol newline so immediately following raw tunnel
// bytes remain unread on the socket.
fn read_forward_response(stream: &mut TcpStream) -> io::Result<String> {
    const MAX_RESPONSE_BYTES: usize = 256;
    let mut response = Vec::new();
    let mut byte = [0; 1];

    while response.len() < MAX_RESPONSE_BYTES {
        if stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "server closed during forward response",
            ));
        }

        response.push(byte[0]);
        if byte[0] == b'\n' {
            return String::from_utf8(response).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "server forward response was not UTF-8",
                )
            });
        }
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "server forward response exceeded 256 bytes",
    ))
}

// Connects both client-side endpoints, then relays without blocking control
// messages such as a future disconnect or additional incoming notification.
fn start_forwarding(
    server_address: SocketAddr,
    session_id: u64,
    local_address: SocketAddr,
) -> io::Result<thread::JoinHandle<()>> {
    // Connect locally first so we do not register a READY forward stream with
    // the server unless there is a local service socket to relay to.
    let local_stream = TcpStream::connect_timeout(&local_address, LOCAL_CONNECT_TIMEOUT)?;
    let forward_stream = open_forward_connection(server_address, session_id)?;

    Ok(thread::spawn(move || {
        if let Err(error) = crate::relay::copy_bidirectional(forward_stream, local_stream) {
            eprintln!("error: local relay failed for session {session_id}: {error}");
        }
    }))
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
        monitor_control_connection, start_forwarding, validate_forward_response,
        validate_handshake_response,
    };
    use std::cell::Cell;
    use std::io::{self, BufRead, BufReader, Cursor, Read, Write};
    use std::net::{Shutdown, SocketAddr, TcpListener};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

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
    fn repeated_incoming_messages_open_repeated_forward_connections() {
        let forward_calls = Cell::new(0);
        let error = monitor_control_connection(Cursor::new(b"INCOMING\nINCOMING\n"), || {
            forward_calls.set(forward_calls.get() + 1);
            Ok(())
        })
        .expect_err("EOF after valid messages should close the session");

        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(forward_calls.get(), 2);
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

    #[test]
    fn forwarding_round_trips_bytes_through_local_service() {
        let local_listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("local listener should bind");
        let local_address = local_listener
            .local_addr()
            .expect("local listener should have an address");
        let local_service = thread::spawn(move || {
            let (mut stream, _) = local_listener
                .accept()
                .expect("local service should accept");
            let mut request = [0; 4];
            stream
                .read_exact(&mut request)
                .expect("local service should read request");
            assert_eq!(&request, b"ping");
            stream
                .write_all(b"pong")
                .expect("local service should write response");
            stream
                .shutdown(Shutdown::Write)
                .expect("local service should close response");
        });

        let server_listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("server listener should bind");
        let server_address = server_listener
            .local_addr()
            .expect("server listener should have an address");
        let forward_server = thread::spawn(move || {
            let (stream, _) = server_listener
                .accept()
                .expect("server should accept forward");
            let mut reader = BufReader::new(stream);
            let mut handshake = String::new();
            reader
                .read_line(&mut handshake)
                .expect("server should read forward handshake");
            assert_eq!(handshake, "FORWARD 42\n");

            let mut stream = reader.into_inner();
            stream
                .write_all(b"READY\nping")
                .expect("server should acknowledge and send request");
            stream
                .shutdown(Shutdown::Write)
                .expect("server should close request");

            let mut response = [0; 4];
            stream
                .read_exact(&mut response)
                .expect("server should read response");
            assert_eq!(&response, b"pong");
        });

        start_forwarding(server_address, 42, local_address)
            .expect("forwarding should start")
            .join()
            .expect("forward relay should finish");
        forward_server.join().expect("forward server should finish");
        local_service.join().expect("local service should finish");
    }

    #[test]
    fn forwarding_checks_local_service_before_opening_forward_connection() {
        let local_address = unavailable_local_address();
        let server_listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("server listener should bind");
        server_listener
            .set_nonblocking(true)
            .expect("server listener should become nonblocking");
        let server_address = server_listener
            .local_addr()
            .expect("server listener should have an address");
        let (accepted_sender, accepted_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_millis(200);

            loop {
                match server_listener.accept() {
                    Ok((stream, _)) => {
                        accepted_sender
                            .send(true)
                            .expect("accept result should be sent");

                        let mut reader = BufReader::new(stream);
                        let mut handshake = String::new();
                        reader
                            .read_line(&mut handshake)
                            .expect("server should read forward handshake");
                        assert_eq!(handshake, "FORWARD 42\n");
                        reader
                            .get_mut()
                            .write_all(b"READY\n")
                            .expect("server should acknowledge forward");
                        return;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            accepted_sender
                                .send(false)
                                .expect("accept result should be sent");
                            return;
                        }

                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("server accept failed: {error}"),
                }
            }
        });

        let error = start_forwarding(server_address, 42, local_address)
            .expect_err("unavailable local service should fail forwarding");

        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
        assert!(!accepted_receiver
            .recv()
            .expect("fake server should report whether it accepted"));
        server.join().expect("fake server should finish");
    }

    fn unavailable_local_address() -> SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("local listener should bind");
        let address = listener
            .local_addr()
            .expect("local listener should have an address");
        drop(listener);

        address
    }
}
