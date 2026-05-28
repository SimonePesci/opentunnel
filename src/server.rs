use std::io;
use std::net::TcpListener;

pub fn run(listen_port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", listen_port))?;

    println!("server listening on {}", listener.local_addr()?);
    println!("press Ctrl-C to stop");

    for stream in listener.incoming() {
        let stream = stream?;
        println!("accepted connection from {}", stream.peer_addr()?);
    }

    Ok(())
}
