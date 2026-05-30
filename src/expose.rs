use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SERVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run(local_port: u16, server_address: SocketAddr) -> io::Result<()> {
    let local_address = SocketAddr::from(([127, 0, 0, 1], local_port));
    let _stream = TcpStream::connect_timeout(&local_address, LOCAL_CONNECT_TIMEOUT)?;

    println!("connected to local service on {local_address}");
    let mut server_stream = TcpStream::connect_timeout(&server_address, SERVER_CONNECT_TIMEOUT)?;

    println!("connected to opentunnel server on {server_address}");
    server_stream.write_all(crate::protocol::expose_handshake(local_port).as_bytes())?;

    println!("sent expose handshake for local port {local_port}");
    println!("tunneling is not implemented yet");

    Ok(())
}
