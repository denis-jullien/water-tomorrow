//! This example uses the RP Pico W board Wifi chip (cyw43).
//! Connects to specified Wifi network and creates a TCP endpoint on port 1234.

#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(stable_features, unknown_lints, async_fn_in_trait)]

mod mqtt;

use cyw43_pio::PioSpi;
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::{TcpSocket};
use embassy_net::{Config, IpAddress, IpEndpoint, Stack, StackResources};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_25, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use embassy_futures::select::select;
use static_cell::make_static;
use mqttrs::*;

use {defmt_rtt as _, panic_probe as _};
use crate::mqtt::{MqttReader, MqttWriter};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

const WIFI_NETWORK: &str = "Things";
const WIFI_PASSWORD: &str = "raclette";
const IP_BROKER: IpAddress = IpAddress::v4(192, 168, 1, 199);
const PORT_BROKER: u16 = 1883;
const USERNAME: &str = "plant";
const PASSWORD: &[u8] = b"plant";


#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static, PIN_23>, PioSpi<'static, PIN_25, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Hello World!");

    let p = embassy_rp::init(Default::default());

    // let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    // let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

    // To make flashing faster for development, you may want to flash the firmwares independently
    // at hardcoded addresses, instead of baking them into the program with `include_bytes!`:
    //     probe-rs download 43439A0.bin --format bin --chip RP2040 --base-address 0x10100000
    //     probe-rs download 43439A0_clm.bin --format bin --chip RP2040 --base-address 0x10140000
    let fw = unsafe { core::slice::from_raw_parts(0x10100000 as *const u8, 230321) };
    let clm = unsafe { core::slice::from_raw_parts(0x10140000 as *const u8, 4752) };

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(&mut pio.common, pio.sm0, pio.irq0, cs, p.PIN_24, p.PIN_29, p.DMA_CH0);

    let state = make_static!(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(wifi_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let config = Config::dhcpv4(Default::default());
    //let config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
    //    address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 69, 2), 24),
    //    dns_servers: Vec::new(),
    //    gateway: Some(Ipv4Address::new(192, 168, 69, 1)),
    //});

    // Generate random seed
    let seed = 0x0123_4567_89ab_cdef; // chosen by fair dice roll. guarenteed to be random.

    // Init network stack
    let stack = &*make_static!(Stack::new(
        net_device,
        config,
        make_static!(StackResources::<2>::new()),
        seed
    ));

    unwrap!(spawner.spawn(net_task(stack)));

    loop {
        //control.join_open(WIFI_NETWORK).await;
        match control.join_wpa2(WIFI_NETWORK, WIFI_PASSWORD).await {
            Ok(_) => break,
            Err(err) => {
                info!("join failed with status={}", err.status);
            }
        }
    }

    // Wait for DHCP, not necessary when using static IP
    info!("waiting for DHCP...");
    while !stack.is_config_up() {
        Timer::after_millis(100).await;
    }
    info!("DHCP is now up!");

    // And now we can use it!
    let broker = IpEndpoint::new(IP_BROKER, PORT_BROKER);

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let mut buf = [0; 4096];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        control.gpio_set(0, false).await;
        info!("Connecting on {}:{}...", IP_BROKER, PORT_BROKER);

        if let Err(e) = socket.connect(broker).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        info!("Connected to {:?}", socket.remote_endpoint());
        control.gpio_set(0, true).await;

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
                //info!("sending");
                let payload = b"Payload";

                match socketw.write_packet(Publish {
                    dup: false,
                    payload,
                    qospid: QosPid::AtMostOnce,
                    retain: true,
                    topic_name: "plop/2"
                }.into()).await{
                    Ok(()) => {},
                    Err(_e) => {
                        break;
                    }
                }

                Timer::after(Duration::from_millis(1_000)).await;
            }
        };

        //unwrap!(spawner.spawn(reader_task(socketr)));

        // If one the the loop break, we have a connection problem
        select(
            puplisher,
            reader,
        ).await;

    }

    //unwrap!(spawner.spawn(reader_task(socketr)));
}

// #[embassy_executor::task]
// async fn reader_task(mut socketr: TcpReader<'static>) {
//     let mut buf = [0; 4096];
//     loop {
//         let n = match socketr.read(&mut buf).await {
//             Ok(0) => {
//                 warn!("read EOF");
//                 break;
//             }
//             Ok(n) => n,
//             Err(e) => {
//                 warn!("read error: {:?}", e);
//                 break;
//             }
//         };
//
//         info!("rxd {}", from_utf8(&buf[..n]).unwrap());
//
//         // Decode one packet. The buffer will advance to the next packet.
//         let rpkt = match decode_slice(&buf[..n]) {
//             Ok(Some(pkt)) => pkt,
//             Ok(None) => {
//                 warn!("no packet");
//                 break;
//             },
//             Err(_e) => {
//                 warn!("decode error");
//                 break;
//             }
//         };
//
//         info!("decoded {}", rpkt.get_type() == PacketType::Connack);
//     }
// }
//
