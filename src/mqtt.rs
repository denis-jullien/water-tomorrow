use defmt::*;
use embassy_net::tcp::{TcpReader, TcpWriter};
use mqttrs::*;
use embedded_io_async::Write;

/// Error returned by TcpSocket read/write functions.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
#[derive(defmt::Format)]
pub enum Error {
    /// The connection was reset.
    ///
    /// This can happen on receiving a RST packet, or on timeout.
    ConnectionReset,
    EncodingError,
    DecodingError,
}

pub trait MqttWriter {
    fn foo(&self);
    async fn write_packet(&mut self, packet: Packet)-> Result<(), Error>;
}

impl<'a> MqttWriter for TcpWriter<'a> {
    fn foo(&self) {
        warn!("foo");
    }
    async fn write_packet(&mut self, packet: Packet<'_>) -> Result<(), Error> {
        let mut buf = [0; 4096];

        let n = match encode_slice(&packet, &mut buf) {
            Ok(n) => n,
            Err(_e) => {
                warn!("encode error");
                return Err(Error::EncodingError);
            }
        };
        match self.write_all(&buf[..n]).await {
            Ok(()) => {}
            Err(e) => {
                warn!("write error: {:?}", e);
                return Err(Error::ConnectionReset);
            }
        };
        Ok(())
    }
}

pub trait MqttReader {

    async fn read_packet<'b>(&'b mut self, buf: &'b mut [u8])-> Result<Packet, Error>;
}

impl<'a> MqttReader for TcpReader<'a> {

    async fn read_packet<'b>(&'b mut self, buf: &'b mut [u8]) -> Result<Packet, Error> {

        let n = match self.read( buf).await {
            Ok(0) => {
                warn!("read EOF");
                return Err(Error::DecodingError);
            }
            Ok(n) => n,
            Err(e) => {
                warn!("read error: {:?}", e);
                return Err(Error::DecodingError);
            }
        };

        info!("rxd {}", &buf[..n]);

        // Decode one packet. The buffer will advance to the next packet.
        let rpkt = match decode_slice(&buf[..n]) {
            Ok(Some(pkt)) => pkt,
            Ok(None) => {
                warn!("no packet");
                return Err(Error::DecodingError);
            },
            Err(_e) => {
                warn!("decode error");
                return Err(Error::DecodingError);
            }
        };
        Ok(rpkt)
    }
}