use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

pub(crate) const UDP_TARGET_PREFIX: &str = "udp://";

pub(crate) fn is_udp_target(target: &str) -> bool {
    target.starts_with(UDP_TARGET_PREFIX)
}

pub(crate) fn udp_target_address(target: &str) -> Option<&str> {
    target.strip_prefix(UDP_TARGET_PREFIX)
}

pub(crate) struct UdpWriter {
    socket: UdpSocket,
    destination: SocketAddr,
}

impl UdpWriter {
    pub(crate) fn open(target: &str) -> io::Result<Self> {
        let address = udp_target_address(target).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("UDP target must start with {UDP_TARGET_PREFIX}"),
            )
        })?;
        let destinations = address.to_socket_addrs()?.collect::<Vec<_>>();
        let destination = destinations
            .iter()
            .find(|destination| destination.is_ipv4())
            .or_else(|| destinations.first())
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, address))?;
        let bind_address = if destination.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind_address)?;
        socket.connect(destination)?;

        Ok(Self {
            socket,
            destination,
        })
    }

    pub(crate) fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        let written = self.socket.send(bytes)?;
        if written != bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!(
                    "UDP socket sent {written} of {} bytes to {}",
                    bytes.len(),
                    self.destination
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{UDP_TARGET_PREFIX, UdpWriter};
    use crate::output::OutputFormat;
    use std::net::UdpSocket;
    use std::time::Duration;

    #[test]
    fn writes_packetmv1_as_one_udp_datagram() {
        assert_format_is_one_datagram(OutputFormat::PacketMv1);
    }

    #[test]
    fn writes_packetacv6_as_one_udp_datagram() {
        assert_format_is_one_datagram(OutputFormat::PacketAcV6);
    }

    fn assert_format_is_one_datagram(format: OutputFormat) {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let target = format!("{UDP_TARGET_PREFIX}{}", receiver.local_addr().unwrap());
        let writer = UdpWriter::open(&target).unwrap();
        let payload = format.encode_dummy_payload().unwrap();

        writer.write_bytes(&payload).unwrap();

        let mut buffer = [0; 64];
        let (length, _) = receiver.recv_from(&mut buffer).unwrap();
        assert_eq!(length, format.packet_len());
        assert_eq!(&buffer[..length], payload);
    }
}
