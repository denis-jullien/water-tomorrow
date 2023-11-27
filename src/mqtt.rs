
use defmt::*;
use mqttrs::*;
use embedded_io_async::Write;
use embassy_net::{IpEndpoint, tcp};
use embassy_net::tcp::{TcpSocket};
use embassy_time::{Duration};

/// Error returned by TcpSocket read/write functions.
#[derive(PartialEq, Clone, Debug)]
pub enum MqttError {
    /// The connection was reset.
    ///
    /// This can happen on receiving a RST packet, or on timeout.
    ConnectionRefused(mqttrs::ConnectReturnCode),
    EncodingError(mqttrs::Error),
    DecodingError(mqttrs::Error),
    NoPacket,
    EOF,
    WrongMessageReceived,
    ConnectError(tcp::ConnectError),
    TcpError(tcp::Error)
}

pub trait MqttWriter {
    fn foo(&self);
    async fn write_packet(&mut self, packet: Packet)-> Result<(), MqttError>;
}

impl MqttWriter for TcpSocket<'_> {
    fn foo(&self) {
        warn!("foo");
    }
    async fn write_packet(&mut self, packet: Packet<'_>) -> Result<(), MqttError> {
        let mut buf = [0; 4096];

        let n = match encode_slice(&packet, &mut buf) {
            Ok(n) => n,
            Err(e) => {
                warn!("encode error");
                return Err(MqttError::EncodingError(e));
            }
        };
        match self.write_all(&buf[..n]).await {
            Ok(()) => {}
            Err(e) => {
                warn!("write error: {:?}", e);
                return Err(MqttError::TcpError(e));
            }
        };
        Ok(())
    }
}

pub trait MqttReader {

    async fn read_packet<'b>(&'b mut self, buf: &'b mut [u8])-> Result<Packet, MqttError>;
}

impl MqttReader for TcpSocket<'_> {

    async fn read_packet<'b>(&'b mut self, buf: &'b mut [u8]) -> Result<Packet, MqttError> {

        let n = match self.read( buf).await {
            Ok(0) => {
                warn!("read EOF");
                return Err(MqttError::EOF);
            }
            Ok(n) => n,
            Err(e) => {
                warn!("read error: {:?}", e);
                return Err(MqttError::TcpError(e));
            }
        };

        info!("rxd {}", &buf[..n]);

        // Decode one packet. The buffer will advance to the next packet.
        let rpkt = match decode_slice(&buf[..n]) {
            Ok(Some(pkt)) => pkt,
            Ok(None) => {
                warn!("no packet");
                return Err(MqttError::NoPacket);
            },
            Err(e) => {
                warn!("decode error");
                return Err(MqttError::DecodingError(e));
            }
        };
        Ok(rpkt)
    }
}

pub struct MqttDriver<'a> {
    socket: TcpSocket<'a>,
    broker: IpEndpoint,
    username: Option<&'a str>,
    password: Option<&'a [u8]>,
}

impl<'a> MqttDriver<'a> {

    pub fn new(mut socket: TcpSocket<'a>, endpoint: IpEndpoint, username: Option<&'a str>, password: Option<&'a [u8]>) ->Self{

        socket.set_keep_alive(Some(Duration::from_secs(10)));

        MqttDriver{
            socket,
            broker:endpoint,
            username,
            password
        }
    }
    pub async fn manage_connection(&mut self) -> Result<(),MqttError>{

        if self.socket.remote_endpoint() == None {

            info!("Connecting on {}:{}...", self.broker.addr, self.broker.port);

            if let Err(e) = self.socket.connect(self.broker).await {
                warn!("accept error: {:?}", e);
                return Err(MqttError::ConnectError(e));
            }

            info!("Connected to {:?}", self.socket.remote_endpoint());

            // Encode an MQTT Connect packet.
            match self.socket.write_packet(Connect {
                protocol: Protocol::MQTT311,
                keep_alive: 30,
                client_id: "plants_wc".into(),
                clean_session: true,
                last_will: None,
                username:self.username,
                password:self.password
            }.into()).await {
                Ok(()) => {},
                Err(e) => {
                    return Err(e);
                }
            }

            let mut buf = [0; 4096];

            match self.socket.read_packet(&mut buf).await {
                Ok(Packet::Connack(c)) => {
                    info!("return session {}, code {}",c.session_present, c.code as u8 );
                    if c.code != ConnectReturnCode::Accepted {
                        return Err(MqttError::ConnectionRefused(c.code))
                    }
                },
                Ok(_pkt) => {
                    warn!("no Connack");
                    return Err(MqttError::WrongMessageReceived);
                }
                Err(e) => {
                    warn!("read error");
                    return Err(e);
                }
            };
        }

        Ok(())
    }

    pub async fn read(&mut self, buf: &mut [u8]) {

            let pkt = match self.socket.read_packet(buf).await {
                Ok(rpkt) => rpkt,
                Err(_e) => {
                    warn!("read error:");
                    return;
                }
            };

            info!("decoded {}", pkt.get_type() == PacketType::Connack);
    }

    pub async fn publish(&mut self, publish:Publish<'_>) -> Result<(),MqttError> {

        self.socket.write_packet(publish.into()).await

    }
}