use std::io;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run(local_port: u16) -> io::Result<()> {
    let local_address = SocketAddr::from(([127, 0, 0, 1], local_port));
    let _stream = TcpStream::connect_timeout(&local_address, LOCAL_CONNECT_TIMEOUT)?;

    println!("connected to local service on {local_address}");
    println!("tunneling is not implemented yet");

    Ok(())
}
