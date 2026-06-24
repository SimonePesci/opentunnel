use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

const CONTROL_POLL_TIMEOUT: Duration = Duration::from_millis(100);

struct ServerState {
    active_exposes: Mutex<Vec<ExposeRegistration>>,
    next_session_id: AtomicU64,
}

struct ExposeRegistration {
    session_id: u64,
    forward_stream: ForwardStreamState,
}

enum ForwardStreamState {
    Empty,
    Requested,
    Waiting(TcpStream),
}

struct ExposeSession<'a> {
    state: &'a ServerState,
    peer_address: SocketAddr,
    local_port: u16,
    session_id: u64,
    // Session ownership keeps the endpoint reserved and closes it on drop.
    tunnel_listener: TcpListener,
    tunnel_address: SocketAddr,
    active_count: usize,
    is_registered: bool,
}

#[derive(Debug)]
enum RegisterExposeError {
    SessionIdsExhausted,
    State(io::Error),
    TunnelPort(io::Error),
}

#[derive(Debug)]
enum AttachForwardError {
    Duplicate,
    NoPendingTunnel,
    Response(io::Error),
    State(io::Error),
    UnknownSession,
}

impl ServerState {
    fn new() -> Self {
        Self {
            active_exposes: Mutex::new(Vec::new()),
            next_session_id: AtomicU64::new(1),
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
        let session_id = self
            .next_session_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| RegisterExposeError::SessionIdsExhausted)?;

        active_exposes.push(ExposeRegistration {
            session_id,
            forward_stream: ForwardStreamState::Empty,
        });

        let active_count = active_exposes.len();
        // Drop the lock before returning ExposeSession, which also borrows self.
        // Rust won't let us return a new borrow of self while the lock is still held.
        drop(active_exposes);

        Ok(ExposeSession {
            state: self,
            peer_address,
            local_port,
            session_id,
            tunnel_listener,
            tunnel_address,
            active_count,
            is_registered: true,
        })
    }

    fn unregister_expose(&self, session_id: u64) -> io::Result<usize> {
        let mut active_exposes = self.lock_active_exposes()?;

        if let Some(index) = active_exposes
            .iter()
            .position(|expose| expose.session_id == session_id)
        {
            active_exposes.swap_remove(index);
        }

        Ok(active_exposes.len())
    }

    // Marks that a tunnel user is waiting and the expose client should open a
    // matching FORWARD stream. We keep this explicit so FORWARD streams cannot
    // be accepted before there is a real tunnel connection to pair with.
    fn request_forward_stream(&self, session_id: u64) -> io::Result<()> {
        let mut active_exposes = self.lock_active_exposes()?;
        let Some(registration) = active_exposes
            .iter_mut()
            .find(|registration| registration.session_id == session_id)
        else {
            return Ok(());
        };

        if !matches!(registration.forward_stream, ForwardStreamState::Empty) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "forward stream request already pending",
            ));
        }

        registration.forward_stream = ForwardStreamState::Requested;
        Ok(())
    }

    // Registers the data stream that will later be paired with this session's
    // pending tunnel user. Returning the stream on failure lets the caller send
    // a rejection before closing it.
    fn attach_forward_stream(
        &self,
        session_id: u64,
        mut stream: TcpStream,
    ) -> Result<(), (AttachForwardError, TcpStream)> {
        let mut active_exposes = match self.lock_active_exposes() {
            Ok(active_exposes) => active_exposes,
            Err(error) => return Err((AttachForwardError::State(error), stream)),
        };
        let Some(registration) = active_exposes
            .iter_mut()
            .find(|registration| registration.session_id == session_id)
        else {
            return Err((AttachForwardError::UnknownSession, stream));
        };

        match &registration.forward_stream {
            ForwardStreamState::Empty => {
                return Err((AttachForwardError::NoPendingTunnel, stream));
            }
            ForwardStreamState::Requested => {}
            ForwardStreamState::Waiting(_) => {
                return Err((AttachForwardError::Duplicate, stream));
            }
        }

        // READY must reach the client before the session loop can relay raw
        // bytes over this stream.
        if let Err(error) = stream.write_all(crate::protocol::forward_ready_response().as_bytes()) {
            return Err((AttachForwardError::Response(error), stream));
        }

        registration.forward_stream = ForwardStreamState::Waiting(stream);
        Ok(())
    }

    // If the forward stream is waiting, take it and return it to the caller.
    // Otherwise, return None. Reset the forward stream state to Empty.
    fn take_forward_stream(&self, session_id: u64) -> io::Result<Option<TcpStream>> {
        let mut active_exposes = self.lock_active_exposes()?;
        let Some(registration) = active_exposes
            .iter_mut()
            .find(|registration| registration.session_id == session_id)
        else {
            return Ok(None);
        };

        let ForwardStreamState::Waiting(_) = registration.forward_stream else {
            return Ok(None);
        };

        match std::mem::replace(&mut registration.forward_stream, ForwardStreamState::Empty) {
            ForwardStreamState::Waiting(stream) => Ok(Some(stream)),
            _ => unreachable!("waiting forward stream was checked before replacement"),
        }
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

    // Identifies this expose session when future data connections arrive.
    fn session_id(&self) -> u64 {
        self.session_id
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

    fn try_take_forward_stream(&self) -> io::Result<Option<TcpStream>> {
        self.state.take_forward_stream(self.session_id)
    }

    fn request_forward_stream(&self) -> io::Result<()> {
        self.state.request_forward_stream(self.session_id)
    }

    fn active_count(&self) -> usize {
        self.active_count
    }

    fn close(mut self) -> io::Result<usize> {
        let active_count = self.state.unregister_expose(self.session_id)?;
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

        if let Err(error) = self.state.unregister_expose(self.session_id) {
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
                        crate::protocol::ok_response(
                            session.tunnel_address(),
                            session.session_id(),
                        )
                        .as_bytes(),
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
                Err(RegisterExposeError::SessionIdsExhausted) => {
                    reader.get_mut().write_all(
                        crate::protocol::error_response("session IDs exhausted").as_bytes(),
                    )?;
                    println!("rejected expose from {peer_address}; session IDs exhausted");
                }
                Err(RegisterExposeError::State(error)) => return Err(error),
            }
        }
        Ok(crate::protocol::Handshake::Forward { session_id }) => {
            let forward_stream = reader.into_inner();

            match state.attach_forward_stream(session_id, forward_stream) {
                Ok(()) => {
                    println!(
                        "registered forward stream from {peer_address} for session {session_id}"
                    );
                }
                Err((AttachForwardError::Duplicate, mut stream)) => {
                    stream.write_all(
                        crate::protocol::error_response("forward stream already registered")
                            .as_bytes(),
                    )?;
                    println!(
                        "rejected duplicate forward stream from {peer_address} for session {session_id}"
                    );
                }
                Err((AttachForwardError::UnknownSession, mut stream)) => {
                    stream.write_all(
                        crate::protocol::error_response("unknown expose session").as_bytes(),
                    )?;
                    println!(
                        "rejected forward stream from {peer_address}; unknown session {session_id}"
                    );
                }
                Err((AttachForwardError::NoPendingTunnel, mut stream)) => {
                    stream.write_all(
                        crate::protocol::error_response("no tunnel connection waiting").as_bytes(),
                    )?;
                    println!(
                        "rejected forward stream from {peer_address}; no tunnel connection waiting for session {session_id}"
                    );
                }
                Err((AttachForwardError::Response(error), _stream)) => return Err(error),
                Err((AttachForwardError::State(error), _stream)) => return Err(error),
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

                // Set the forward stream state to REQUESTED and proceed
                // to send INCOMING connection message to the expose client.
                session.request_forward_stream()?;
                reader
                    .get_mut()
                    .write_all(crate::protocol::incoming_connection_message().as_bytes())?;

                // Keep one connection open until the data-forwarding protocol
                // can hand it to the expose client.
                pending_tunnel_connection = Some(stream);
            }
        }

        // Take the forward stream, and set the forward stream state to Empty.
        if let Some(forward_stream) = session.try_take_forward_stream()? {
            let tunnel_stream = pending_tunnel_connection
                .take()
                .ok_or_else(|| io::Error::other("forward stream arrived without tunnel user"))?;
            let session_id = session.session_id();

            // Relaying blocks until both peers close, so keep control-session
            // monitoring on this thread and move byte copying to a worker.
            thread::spawn(move || {
                if let Err(error) = crate::relay::copy_bidirectional(tunnel_stream, forward_stream)
                {
                    eprintln!("error: relay failed for session {session_id}: {error}");
                }
            });
            println!("started server relay for session {session_id}");
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
    use super::{AttachForwardError, ServerState};
    use std::io::{self, BufRead, BufReader};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

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
        assert_ne!(first_session.session_id(), second_session.session_id());
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
        let (_stream, peer_address) = wait_for_tunnel_connection(&session);

        assert_eq!(
            peer_address,
            connector_stream
                .local_addr()
                .expect("connector should have a local address")
        );
    }

    #[test]
    fn active_session_accepts_only_one_forward_stream() {
        let state = ServerState::new();
        let session = state
            .register_expose_session(peer_address(41000), 3000)
            .expect("expose should register");
        state
            .request_forward_stream(session.session_id())
            .expect("forward request should be tracked");
        let (_first_client, first_server) = connected_stream_pair();

        state
            .attach_forward_stream(session.session_id(), first_server)
            .expect("first forward stream should attach");

        let (_second_client, second_server) = connected_stream_pair();
        assert!(matches!(
            state.attach_forward_stream(session.session_id(), second_server),
            Err((AttachForwardError::Duplicate, _))
        ));
    }

    #[test]
    fn attached_forward_stream_is_ready_and_claimed_once() {
        let state = ServerState::new();
        let session = state
            .register_expose_session(peer_address(41000), 3000)
            .expect("expose should register");
        state
            .request_forward_stream(session.session_id())
            .expect("forward request should be tracked");
        let (client, server) = connected_stream_pair();

        state
            .attach_forward_stream(session.session_id(), server)
            .expect("forward stream should attach");

        let mut response = String::new();
        BufReader::new(client)
            .read_line(&mut response)
            .expect("ready response should be readable");
        assert_eq!(response, "READY\n");
        assert!(session
            .try_take_forward_stream()
            .expect("first claim should succeed")
            .is_some());
        assert!(session
            .try_take_forward_stream()
            .expect("second claim should succeed")
            .is_none());
    }

    #[test]
    fn forward_stream_requires_waiting_tunnel_connection() {
        let state = ServerState::new();
        let session = state
            .register_expose_session(peer_address(41000), 3000)
            .expect("expose should register");
        let (_client, server) = connected_stream_pair();

        assert!(matches!(
            state.attach_forward_stream(session.session_id(), server),
            Err((AttachForwardError::NoPendingTunnel, _))
        ));
    }

    #[test]
    fn claimed_forward_stream_allows_next_forward_request() {
        let state = ServerState::new();
        let session = state
            .register_expose_session(peer_address(41000), 3000)
            .expect("expose should register");

        state
            .request_forward_stream(session.session_id())
            .expect("first forward request should be tracked");
        let (_first_client, first_server) = connected_stream_pair();
        state
            .attach_forward_stream(session.session_id(), first_server)
            .expect("first forward stream should attach");
        assert!(session
            .try_take_forward_stream()
            .expect("first claim should succeed")
            .is_some());

        state
            .request_forward_stream(session.session_id())
            .expect("second forward request should be tracked");
        let (_second_client, second_server) = connected_stream_pair();

        state
            .attach_forward_stream(session.session_id(), second_server)
            .expect("second forward stream should attach");
        assert!(session
            .try_take_forward_stream()
            .expect("second claim should succeed")
            .is_some());
    }

    #[test]
    fn unknown_session_rejects_forward_stream() {
        let state = ServerState::new();
        let (_client, server) = connected_stream_pair();

        assert!(matches!(
            state.attach_forward_stream(999, server),
            Err((AttachForwardError::UnknownSession, _))
        ));
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

    fn connected_stream_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("pair listener should bind");
        let address = listener
            .local_addr()
            .expect("pair listener should have an address");
        let client = TcpStream::connect(address).expect("pair client should connect");
        let (server, _) = listener.accept().expect("pair server should accept");

        (client, server)
    }

    fn wait_for_tunnel_connection(session: &super::ExposeSession<'_>) -> (TcpStream, SocketAddr) {
        let deadline = Instant::now() + Duration::from_millis(200);

        loop {
            match session
                .try_accept_tunnel_connection()
                .expect("listener check should succeed")
            {
                Some(connection) => return connection,
                None if Instant::now() >= deadline => {
                    panic!("session should accept tunnel connection")
                }
                None => thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}
