use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

pub fn run(listen_port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", listen_port))?;

    println!("server listening on {}", listener.local_addr()?);
    println!("press Ctrl-C to stop");

    for stream in listener.incoming() {
        let stream = stream?;

        // Reading from a client can block, so keep the listener free to accept.
        thread::spawn(move || {
            if let Err(error) = handle_connection(stream) {
                eprintln!("error: connection failed: {error}");
            }
        });
    }

    Ok(())
}

fn handle_connection(stream: TcpStream) -> io::Result<()> {
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
            reader
                .get_mut()
                .write_all(crate::protocol::ok_response().as_bytes())?;
            println!("registered expose from {peer_address} for local port {local_port}");
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
