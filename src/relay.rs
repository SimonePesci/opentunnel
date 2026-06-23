use std::io;
use std::net::{Shutdown, TcpStream};
use std::thread;

// Copies bytes in both directions until each peer closes its write side.
pub fn copy_bidirectional(left: TcpStream, right: TcpStream) -> io::Result<()> {
    let mut left_reader = left.try_clone()?;
    let mut right_reader = right.try_clone()?;
    let mut left_writer = left;
    let mut right_writer = right;

    let left_to_right = thread::spawn(move || {
        let result = io::copy(&mut left_reader, &mut right_writer);
        let _ = right_writer.shutdown(Shutdown::Write);
        result
    });

    let right_to_left = io::copy(&mut right_reader, &mut left_writer);
    let _ = left_writer.shutdown(Shutdown::Write);
    let left_to_right = left_to_right
        .join()
        .map_err(|_| io::Error::other("relay worker thread panicked"))?;

    right_to_left?;
    left_to_right?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::copy_bidirectional;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn copies_bytes_in_both_directions() {
        let (mut left_client, left_relay) = connected_stream_pair();
        let (mut right_client, right_relay) = connected_stream_pair();
        left_client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("left timeout should configure");
        right_client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("right timeout should configure");

        let relay = thread::spawn(move || copy_bidirectional(left_relay, right_relay));

        left_client
            .write_all(b"left to right")
            .expect("left write should succeed");
        let mut from_left = [0; 13];
        right_client
            .read_exact(&mut from_left)
            .expect("right should receive left bytes");
        assert_eq!(&from_left, b"left to right");

        right_client
            .write_all(b"right to left")
            .expect("right write should succeed");
        let mut from_right = [0; 13];
        left_client
            .read_exact(&mut from_right)
            .expect("left should receive right bytes");
        assert_eq!(&from_right, b"right to left");

        left_client
            .shutdown(Shutdown::Write)
            .expect("left write side should close");
        right_client
            .shutdown(Shutdown::Write)
            .expect("right write side should close");

        relay
            .join()
            .expect("relay thread should finish")
            .expect("relay should finish cleanly");
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
}
