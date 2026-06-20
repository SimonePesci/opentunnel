use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

struct ServerState {
    active_exposes: Mutex<Vec<ExposeRegistration>>,
}

struct ExposeRegistration {
    peer_address: SocketAddr,
    local_port: u16,
    // Retaining the listener reserves the tunnel port until the session ends.
    _tunnel_listener: TcpListener,
}

struct ExposeSession<'a> {
    state: &'a ServerState,
    peer_address: SocketAddr,
    local_port: u16,
    active_count: usize,
    is_registered: bool,
}

#[derive(Debug)]
enum RegisterExposeError {
    DuplicateLocalPort,
    State(io::Error),
    TunnelPort(io::Error),
}

impl ServerState {
    fn new() -> Self {
        Self {
            active_exposes: Mutex::new(Vec::new()),
        }
    }

    // Registers a new expose session. Returns DuplicateLocalPort if the same
    // local port is already active. Binds a tunnel listener to reserve the port
    // so no other process (or another expose) can steal it while the session is
    // alive.
    fn register_expose_session(
        &self,
        peer_address: SocketAddr,
        local_port: u16,
    ) -> Result<ExposeSession<'_>, RegisterExposeError> {
        let mut active_exposes = self
            .lock_active_exposes()
            .map_err(RegisterExposeError::State)?;

        if active_exposes
            .iter()
            .any(|expose| expose.local_port == local_port)
        {
            return Err(RegisterExposeError::DuplicateLocalPort);
        }

        let tunnel_listener = TcpListener::bind(("127.0.0.1", local_port))
            .map_err(RegisterExposeError::TunnelPort)?;

        active_exposes.push(ExposeRegistration {
            peer_address,
            local_port,
            _tunnel_listener: tunnel_listener,
        });

        let active_count = active_exposes.len();
        // Drop the lock before returning ExposeSession, which also borrows self.
        // Rust won't let us return a new borrow of self while the lock is still held.
        drop(active_exposes);

        Ok(ExposeSession {
            state: self,
            peer_address,
            local_port,
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
                    reader
                        .get_mut()
                        .write_all(crate::protocol::ok_response().as_bytes())?;

                    println!(
                        "registered expose from {peer_address} for local port {local_port}; active exposes: {}",
                        session.active_count()
                    );
                    wait_for_expose_disconnect(&mut reader)?;
                    let active_count = session.close()?;

                    println!(
                        "expose disconnected from {peer_address} for local port {local_port}; active exposes: {active_count}"
                    );
                }
                Err(RegisterExposeError::DuplicateLocalPort) => {
                    reader.get_mut().write_all(
                        crate::protocol::error_response("local port already exposed").as_bytes(),
                    )?;

                    println!(
                        "rejected duplicate expose from {peer_address} for local port {local_port}"
                    );
                }
                Err(RegisterExposeError::TunnelPort(error)) => {
                    reader.get_mut().write_all(
                        crate::protocol::error_response("tunnel port unavailable").as_bytes(),
                    )?;

                    println!(
                        "rejected expose from {peer_address}; tunnel port {local_port} is unavailable: {error}"
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

// Blocks until the remote end closes its side of the TCP connection. When the
// expose client disconnects (Ctrl-C, crash, network drop), read_line returns
// Ok(0), which signals EOF in TCP.
fn wait_for_expose_disconnect(reader: &mut BufReader<TcpStream>) -> io::Result<()> {
    let mut message = String::new();

    loop {
        message.clear();

        if reader.read_line(&mut message)? == 0 {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RegisterExposeError, ServerState};
    use std::io;
    use std::net::{SocketAddr, TcpListener};

    #[test]
    fn active_expose_session_rejects_duplicate_local_port() {
        let state = ServerState::new();
        let first_peer = peer_address(41000);
        let second_peer = peer_address(41001);
        let local_port = available_port();

        let _session = state
            .register_expose_session(first_peer, local_port)
            .expect("first expose should register");

        assert!(matches!(
            state.register_expose_session(second_peer, local_port),
            Err(RegisterExposeError::DuplicateLocalPort)
        ));
    }

    #[test]
    fn active_expose_session_reserves_tunnel_port() {
        let state = ServerState::new();
        let local_port = available_port();

        let _session = state
            .register_expose_session(peer_address(41000), local_port)
            .expect("expose should register");

        let error = TcpListener::bind(("127.0.0.1", local_port))
            .expect_err("active expose should reserve its tunnel port");

        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    }

    #[test]
    fn occupied_tunnel_port_rejects_expose() {
        let state = ServerState::new();
        let occupied_listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("test listener should bind");
        let occupied_port = occupied_listener
            .local_addr()
            .expect("test listener should have an address")
            .port();

        assert!(matches!(
            state.register_expose_session(peer_address(41000), occupied_port),
            Err(RegisterExposeError::TunnelPort(error))
                if error.kind() == io::ErrorKind::AddrInUse
        ));
    }

    #[test]
    fn dropped_expose_session_releases_tunnel_port() {
        let state = ServerState::new();
        let first_peer = peer_address(41000);
        let second_peer = peer_address(41001);
        let local_port = available_port();

        let session = state
            .register_expose_session(first_peer, local_port)
            .expect("first expose should register");
        drop(session);

        assert!(state
            .register_expose_session(second_peer, local_port)
            .is_ok());
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
