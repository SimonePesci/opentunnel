use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

const CONTROL_POLL_TIMEOUT: Duration = Duration::from_millis(100);

struct ServerState {
    active_exposes: Mutex<Vec<ExposeRegistration>>,
}

struct ExposeRegistration {
    peer_address: SocketAddr,
    local_port: u16,
}

struct ExposeSession<'a> {
    state: &'a ServerState,
    peer_address: SocketAddr,
    local_port: u16,
    // Session ownership keeps the endpoint reserved and closes it on drop.
    tunnel_listener: TcpListener,
    tunnel_address: SocketAddr,
    active_count: usize,
    is_registered: bool,
}

#[derive(Debug)]
enum RegisterExposeError {
    State(io::Error),
    TunnelPort(io::Error),
}

impl ServerState {
    fn new() -> Self {
        Self {
            active_exposes: Mutex::new(Vec::new()),
        }
    }

    // Registers a new expose session and allocates an available server-side
    // tunnel port. The client's local port identifies its target service; it
    // must not constrain which port is available on the server.
    fn register_expose_session(
        &self,
        peer_address: SocketAddr,
        local_port: u16,
    ) -> Result<ExposeSession<'_>, RegisterExposeError> {
        let mut active_exposes = self
            .lock_active_exposes()
            .map_err(RegisterExposeError::State)?;

        // Port zero delegates uniqueness to the OS, which owns the server-side
        // port namespace and can allocate safely across concurrent sessions.
        let tunnel_listener =
            TcpListener::bind(("127.0.0.1", 0)).map_err(RegisterExposeError::TunnelPort)?;
        tunnel_listener
            .set_nonblocking(true)
            .map_err(RegisterExposeError::TunnelPort)?;
        let tunnel_address = tunnel_listener
            .local_addr()
            .map_err(RegisterExposeError::TunnelPort)?;

        active_exposes.push(ExposeRegistration {
            peer_address,
            local_port,
        });

        let active_count = active_exposes.len();
        // Drop the lock before returning ExposeSession, which also borrows self.
        // Rust won't let us return a new borrow of self while the lock is still held.
        drop(active_exposes);

        Ok(ExposeSession {
            state: self,
            peer_address,
            local_port,
            tunnel_listener,
            tunnel_address,
            active_count,
            is_registered: true,
        })
    }

    fn unregister_expose(&self, peer_address: SocketAddr, local_port: u16) -> io::Result<usize> {
        let mut active_exposes = self.lock_active_exposes()?;

        if let Some(index) = active_exposes.iter().position(|expose| {
            expose.peer_address == peer_address && expose.local_port == local_port
        }) {
            active_exposes.swap_remove(index);
        }

        Ok(active_exposes.len())
    }

    fn lock_active_exposes(&self) -> io::Result<MutexGuard<'_, Vec<ExposeRegistration>>> {
        self.active_exposes
            .lock()
            .map_err(|_| io::Error::other("server state lock poisoned"))
    }
}

impl ExposeSession<'_> {
    // Returns the actual listener address advertised to the expose client.
    fn tunnel_address(&self) -> SocketAddr {
        self.tunnel_address
    }

    // Checks for an incoming tunnel user without blocking control-session
    // monitoring on the same connection thread.
    fn try_accept_tunnel_connection(&self) -> io::Result<Option<(TcpStream, SocketAddr)>> {
        match self.tunnel_listener.accept() {
            Ok(connection) => Ok(Some(connection)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn active_count(&self) -> usize {
        self.active_count
    }

    fn close(mut self) -> io::Result<usize> {
        let active_count = self
            .state
            .unregister_expose(self.peer_address, self.local_port)?;
        self.is_registered = false;

        Ok(active_count)
    }
}

// Safety net: if the caller drops the session without calling close() first
// (e.g. early return via ?, a panic), the session still unregisters itself.
// Without this, the server would leak stale expose entries and never free the
// tunnel port.
impl Drop for ExposeSession<'_> {
    fn drop(&mut self) {
        if !self.is_registered {
            return;
        }

        if let Err(error) = self
            .state
            .unregister_expose(self.peer_address, self.local_port)
        {
            eprintln!(
                "error: failed to unregister expose from {} for local port {}: {error}",
                self.peer_address, self.local_port
            );
        }
    }
}

pub fn run(listen_port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", listen_port))?;
    let state = Arc::new(ServerState::new());

    println!("server listening on {}", listener.local_addr()?);
    println!("press Ctrl-C to stop");

    for stream in listener.incoming() {
        let stream = stream?;
        let state = Arc::clone(&state);

        // Spawn a thread per connection so one blocked read doesn't stall the
        // accept loop.
        thread::spawn(move || {
            if let Err(error) = handle_connection(stream, state) {
                eprintln!("error: connection failed: {error}");
            }
        });
    }

    Ok(())
}

fn handle_connection(stream: TcpStream, state: Arc<ServerState>) -> io::Result<()> {
    let peer_address = stream.peer_addr()?;
    let mut reader = BufReader::new(stream);
    let mut message = String::new();
    let bytes_read = reader.read_line(&mut message)?;

    if bytes_read == 0 {
        println!("accepted connection from {peer_address} without handshake");
        return Ok(());
    }

    match crate::protocol::parse_handshake(message.trim_end()) {
        Ok(crate::protocol::Handshake::Expose { local_port }) => {
            match state.register_expose_session(peer_address, local_port) {
                Ok(session) => {
                    reader.get_mut().write_all(
                        crate::protocol::ok_response(session.tunnel_address()).as_bytes(),
                    )?;

                    println!(
                        "registered expose from {peer_address} for local port {local_port}; active exposes: {}",
                        session.active_count()
                    );
                    reader
                        .get_mut()
                        .set_read_timeout(Some(CONTROL_POLL_TIMEOUT))?;
                    wait_for_expose_activity(&mut reader, &session)?;
                    let active_count = session.close()?;

                    println!(
                        "expose disconnected from {peer_address} for local port {local_port}; active exposes: {active_count}"
                    );
                }
                Err(RegisterExposeError::TunnelPort(error)) => {
                    reader.get_mut().write_all(
                        crate::protocol::error_response("tunnel port unavailable").as_bytes(),
                    )?;

                    println!(
                        "rejected expose from {peer_address} for local port {local_port}; failed to allocate tunnel port: {error}"
                    );
                }
                Err(RegisterExposeError::State(error)) => return Err(error),
            }
        }
        Err(error) => {
            let error_message = crate::protocol::describe_parse_error(error);
            reader
                .get_mut()
                .write_all(crate::protocol::error_response(error_message).as_bytes())?;
            println!("invalid handshake from {peer_address}: {error_message}");
        }
    }

    Ok(())
}

// Monitors both sides of an expose session without letting either blocking
// socket operation hide activity from the other side.
fn wait_for_expose_activity(
    reader: &mut BufReader<TcpStream>,
    session: &ExposeSession<'_>,
) -> io::Result<()> {
    let mut message = String::new();
    let mut pending_tunnel_connection = None;

    loop {
        if pending_tunnel_connection.is_none() {
            if let Some((stream, peer_address)) = session.try_accept_tunnel_connection()? {
                println!(
                    "accepted tunnel connection from {peer_address} on {}",
                    session.tunnel_address()
                );
                reader
                    .get_mut()
                    .write_all(crate::protocol::incoming_connection_message().as_bytes())?;

                // Keep one connection open until the data-forwarding protocol
                // can hand it to the expose client.
                pending_tunnel_connection = Some(stream);
            }
        }

        message.clear();

        match reader.read_line(&mut message) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ServerState;
    use std::io;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread;

    #[test]
    fn separate_clients_can_expose_the_same_local_port() {
        let state = ServerState::new();
        let first_peer = peer_address(41000);
        let second_peer = peer_address(41001);
        let local_port = available_port();

        let first_session = state
            .register_expose_session(first_peer, local_port)
            .expect("first expose should register");
        let second_session = state
            .register_expose_session(second_peer, local_port)
            .expect("second expose should register");

        assert_ne!(
            first_session.tunnel_address(),
            second_session.tunnel_address()
        );
    }

    #[test]
    fn active_expose_session_reserves_tunnel_port() {
        let state = ServerState::new();
        let local_port = available_port();

        let session = state
            .register_expose_session(peer_address(41000), local_port)
            .expect("expose should register");

        let error = TcpListener::bind(session.tunnel_address())
            .expect_err("active expose should reserve its tunnel port");

        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn expose_session_reports_bound_tunnel_address() {
        let state = ServerState::new();
        let local_port = available_port();

        let session = state
            .register_expose_session(peer_address(41000), local_port)
            .expect("expose should register");

        assert_eq!(session.tunnel_address().ip(), peer_address(0).ip());
        assert_ne!(session.tunnel_address().port(), 0);
    }

    #[test]
    fn expose_session_accepts_tunnel_connection() {
        let state = ServerState::new();
        let session = state
            .register_expose_session(peer_address(41000), 3000)
            .expect("expose should register");
        let tunnel_address = session.tunnel_address();

        let connector_stream = thread::spawn(move || {
            TcpStream::connect(tunnel_address).expect("tunnel client should connect")
        })
        .join()
        .expect("connector thread should finish");
        let (_stream, peer_address) = session
            .try_accept_tunnel_connection()
            .expect("listener check should succeed")
            .expect("session should accept tunnel connection");

        assert_eq!(
            peer_address,
            connector_stream
                .local_addr()
                .expect("connector should have a local address")
        );
    }

    #[test]
    fn occupied_local_port_does_not_block_tunnel_allocation() {
        let state = ServerState::new();
        let occupied_listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("test listener should bind");
        let occupied_port = occupied_listener
            .local_addr()
            .expect("test listener should have an address")
            .port();

        let session = state
            .register_expose_session(peer_address(41000), occupied_port)
            .expect("server should allocate a separate tunnel port");

        assert_ne!(session.tunnel_address().port(), occupied_port);
    }

    #[test]
    fn dropped_expose_session_releases_tunnel_port() {
        let state = ServerState::new();
        let local_port = available_port();

        let session = state
            .register_expose_session(peer_address(41000), local_port)
            .expect("first expose should register");
        let tunnel_address = session.tunnel_address();
        drop(session);

        assert!(TcpListener::bind(tunnel_address).is_ok());
    }

    fn peer_address(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn available_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test listener should bind");

        listener
            .local_addr()
            .expect("test listener should have an address")
            .port()
    }
}
