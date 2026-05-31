use std::io::{self, BufRead, BufReader, Write};
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

    if !crate::protocol::is_ok_response(&response) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected server response: {}", response.trim_end()),
        ));
    }

    println!("server accepted expose handshake");
    keep_control_connection(reader)
}

fn keep_control_connection(_reader: BufReader<TcpStream>) -> io::Result<()> {
    println!("expose session active; press Ctrl-C to stop");
    println!("tunneling is not implemented yet");

    // Holding the reader keeps the TCP control connection open for the server.
    loop {
        thread::park();
    }
}
