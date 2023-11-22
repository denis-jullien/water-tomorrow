use cyw43::NetDriver;
use defmt::*;
use embassy_net::tcp::{TcpReader, TcpWriter};
use mqttrs::*;
use embedded_io_async::Write;
use embassy_net::{ IpAddress, IpEndpoint, Stack};
use embassy_net::tcp::{TcpSocket};
use embassy_time::{Duration};
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::{Receiver};

const IP_BROKER: IpAddress = IpAddress::v4(192, 168, 1, 199);
const PORT_BROKER: u16 = 1883;
const USERNAME: &str = "plant";
const PASSWORD: &[u8] = b"plant";


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

pub async fn run(stack: &Stack<NetDriver<'_>>, receiver: Receiver<'_, NoopRawMutex, Publish<'_>, 5>){
    // And now we can use it!
    let broker = IpEndpoint::new(IP_BROKER, PORT_BROKER);

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let mut buf = [0; 4096];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("Connecting on {}:{}...", IP_BROKER, PORT_BROKER);

        if let Err(e) = socket.connect(broker).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        info!("Connected to {:?}", socket.remote_endpoint());

        let (mut socketr, mut socketw) = socket.split();

        // Encode an MQTT Connect packet.
        match socketw.write_packet(Connect {
            protocol: Protocol::MQTT311,
            keep_alive: 30,
            client_id: "doc_client".into(),
            clean_session: true,
            last_will: None,
            username: Some(USERNAME),
            password: Some(PASSWORD)
        }.into()).await {
            Ok(()) => {},
            Err(_e) => {
                continue;
            }
        }

        match socketr.read_packet(&mut buf).await {
            Ok(pkt) if pkt.get_type() == PacketType::Connack => {},
            Ok(_pkt) => {
                warn!("no Connack");
                continue;
            }
            Err(e) => {
                warn!("read error: {:?}", e);
                continue;
            }
        };

        let reader = async {

            loop {
                let pkt = match socketr.read_packet(&mut buf).await {
                    Ok(pkt) => pkt,
                    Err(e) => {
                        warn!("read error: {:?}", e);
                        break;
                    }
                };

                info!("decoded {}", pkt.get_type() == PacketType::Connack);
            }
        };

        let puplisher = async {
            loop {
                let rx = receiver.receive().await;

                match socketw.write_packet(rx.into()).await{
                    Ok(()) => {},
                    Err(_e) => {
                        break;
                    }
                }

                //Timer::after(Duration::from_millis(1_000)).await;
            }
        };

        //unwrap!(spawner.spawn(reader_task(socketr)));

        // If one the the loop break, we have a connection problem
        select(
            puplisher,
            reader,
        ).await;

    }
}