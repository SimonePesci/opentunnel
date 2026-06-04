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
}

enum RegisterExposeError {
    DuplicateLocalPort,
    State(io::Error),
}

impl ServerState {
    fn new() -> Self {
        Self {
            active_exposes: Mutex::new(Vec::new()),
        }
    }

    fn register_expose(
        &self,
        peer_address: SocketAddr,
        local_port: u16,
    ) -> Result<usize, RegisterExposeError> {
        let mut active_exposes = self
            .lock_active_exposes()
            .map_err(RegisterExposeError::State)?;

        if active_exposes
            .iter()
            .any(|expose| expose.local_port == local_port)
        {
            return Err(RegisterExposeError::DuplicateLocalPort);
        }

        active_exposes.push(ExposeRegistration {
            peer_address,
            local_port,
        });

        Ok(active_exposes.len())
    }

    fn unregister_expose(&self, peer_address: SocketAddr, local_port: u16) -> io::Result<usize> {
        let mut active_exposes = self.lock_active_exposes()?;

        if let Some(index) = active_exposes
            .iter()
            .position(|expose| {
                expose.peer_address == peer_address && expose.local_port == local_port
            })
        {
            active_exposes.swap_remove(index);
        }

        Ok(active_exposes.len())
    }

    fn lock_active_exposes(&self) -> io::Result<MutexGuard<'_, Vec<ExposeRegistration>>> {
        self.active_exposes
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "server state lock poisoned"))
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

        // Reading from a client can block, so keep the listener free to accept.
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
            match state.register_expose(peer_address, local_port) {
                Ok(active_count) => {
                    reader
                        .get_mut()
                        .write_all(crate::protocol::ok_response().as_bytes())?;

                    println!(
                        "registered expose from {peer_address} for local port {local_port}; active exposes: {active_count}"
                    );
                    wait_for_expose_disconnect(reader, state, peer_address, local_port)?;
                }
                Err(RegisterExposeError::DuplicateLocalPort) => {
                    reader
                        .get_mut()
                        .write_all(crate::protocol::error_response().as_bytes())?;

                    println!(
                        "rejected duplicate expose from {peer_address} for local port {local_port}"
                    );
                }
                Err(RegisterExposeError::State(error)) => return Err(error),
            }
        }
        Err(error) => {
            reader
                .get_mut()
                .write_all(crate::protocol::error_response().as_bytes())?;
            println!(
                "invalid handshake from {peer_address}: {}",
                crate::protocol::describe_parse_error(error)
            );
        }
    }

    Ok(())
}

fn wait_for_expose_disconnect(
    mut reader: BufReader<TcpStream>,
    state: Arc<ServerState>,
    peer_address: SocketAddr,
    local_port: u16,
) -> io::Result<()> {
    let mut message = String::new();

    loop {
        message.clear();

        if reader.read_line(&mut message)? == 0 {
            let active_count = state.unregister_expose(peer_address, local_port)?;

            println!(
                "expose disconnected from {peer_address} for local port {local_port}; active exposes: {active_count}"
            );
            return Ok(());
        }
    }
}
