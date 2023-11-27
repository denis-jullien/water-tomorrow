//! This example uses the RP Pico W board Wifi chip (cyw43).
//! Connects to specified Wifi network and creates a TCP endpoint on port 1234.

#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(stable_features, unknown_lints, async_fn_in_trait)]


mod mqtt;

use u8g2_fonts::fonts;
use u8g2_fonts::FontRenderer;
use u8g2_fonts::types::*;

use cyw43_pio::PioSpi;
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::{Config, IpEndpoint, Stack, StackResources, IpAddress};
use embassy_net::tcp::TcpSocket;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Input, Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_25, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use mqttrs::{Publish, QosPid};

use static_cell::make_static;

use embedded_graphics::{
    mono_font::MonoTextStyleBuilder,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle},
    text::{Baseline, Text, TextStyleBuilder},
};
use epd_waveshare::{epd2in9_v2::*, prelude::*};
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_rp::gpio::{Pull};
use embassy_time::Delay;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use core::cell::RefCell;
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
use embedded_graphics::primitives::Arc;

use {defmt_rtt as _, panic_probe as _};

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

// Spi
    let e_dc = Output::new(p.PIN_8, Level::Low);
    let e_cs = p.PIN_9;
    let mosi = p.PIN_11;
    let clk = p.PIN_10;
    let e_rst = Output::new(p.PIN_12, Level::Low);
    let e_busy = Input::new(p.PIN_13, Pull::None);

    // let tp_rst = p.PIN_16;
    // let tp_int = p.PIN_17;

    // create SPI
    let mut display_config = spi::Config::default();
    display_config.frequency = 64_000_000;
    display_config.phase = spi::Phase::CaptureOnSecondTransition;
    display_config.polarity = spi::Polarity::IdleHigh;


    let spi: Spi<'_, _, Blocking> = Spi::new_blocking_txonly(p.SPI1, clk, mosi, display_config.clone());
    let spi_bus: Mutex<NoopRawMutex, _> = Mutex::new(RefCell::new(spi));
    let mut display_spi = SpiDeviceWithConfig::new(&spi_bus, Output::new(e_cs, Level::High), display_config);
    let mut epd = Epd2in9::new(&mut display_spi, e_busy, e_dc, e_rst, &mut Delay, None).expect("eink initalize error");

    // Use display graphics from embedded-graphics
    let mut display = Display2in9::default();
    display.set_rotation(DisplayRotation::Rotate90);
    display.clear(Color::White).ok();

    // Use embedded graphics for drawing a line
    let _ = Line::new(Point::new(10, 10), Point::new(295, 127))
        .into_styled(PrimitiveStyle::with_stroke(Color::Black, 1))
        .draw(&mut display);

    let _ = Arc::with_center(Point::new(50, display.bounding_box().bottom_right().unwrap().y-50), 80, -90.0.deg(), 270.0.deg())
        .into_styled(PrimitiveStyle::with_stroke(Color::Black, 3))
        .draw(&mut display);

    let font = FontRenderer::new::<fonts::u8g2_font_inr30_mf >();
    let font2 = FontRenderer::new::<fonts::u8g2_font_courR12_tf>();

    let text = "16:30";

    let _ = font.render_aligned(
        text,
        display.bounding_box().center() + Point::new(0, 0),
        VerticalPosition::Baseline,
        HorizontalAlignment::Center,
        FontColor::Transparent(Color::Black),
        &mut display,
    );

    let _ = font2.render_aligned(
        "Vendredi 24 novembre",
        Point::new(display.bounding_box().center().x, 15),
        VerticalPosition::Baseline,
        HorizontalAlignment::Center,
        FontColor::Transparent(Color::Black),
        &mut display,
    );


    let style = MonoTextStyleBuilder::new()
        .font(&embedded_graphics::mono_font::ascii::FONT_9X18)
        .text_color(Color::Black)
        .background_color(Color::White)
        .build();

    let text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();

    //let _ = Text::with_text_style("text", Point::new(5, 50), style, text_style).draw(&mut display);

    epd.update_and_display_frame(&mut display_spi, display.buffer(), &mut Delay)
        .expect("display frame new graphics");

// Wifi
    info!("Wifi!");

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
    let piospi = PioSpi::new(&mut pio.common, pio.sm0, pio.irq0, cs, p.PIN_24, p.PIN_29, p.DMA_CH0);

    let state = make_static!(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, piospi, fw).await;
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

    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    let socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    let broker = IpEndpoint::new(IP_BROKER, PORT_BROKER);

    let mut mqtt = mqtt::MqttDriver::new(socket, broker, Some(USERNAME), Some(PASSWORD));

    info!("Connection !");

    let mut temperature:f32 = 1.2;

    loop {
        let _ = mqtt.manage_connection().await;

        let mut buffer = ryu::Buffer::new();
        temperature += 0.1;

        info!("loop {}", temperature);

        let _ = mqtt.publish(Publish {
            dup: false,
            payload: buffer.format(temperature).as_bytes(),
            qospid: QosPid::AtMostOnce,
            retain: false,
            topic_name: "indoor_plants/wc/plant1/temperature"
        }).await;

        let _ = mqtt.publish(Publish {
            dup: false,
            payload: buffer.format(42.0).as_bytes(),
            qospid: QosPid::AtMostOnce,
            retain: false,
            topic_name: "indoor_plants/wc/plant1/humidity"
        }).await;

        let _ = mqtt.publish(Publish {
            dup: false,
            payload: buffer.format(69.0).as_bytes(),
            qospid: QosPid::AtMostOnce,
            retain: false,
            topic_name: "indoor_plants/wc/relative_humidity"
        }).await;

        let _ = Text::with_text_style(buffer.format(temperature), Point::new(200, 50), style, text_style).draw(&mut display);
        epd
            .update_and_display_new_frame(&mut display_spi, display.buffer(), &mut Delay)
            .expect("display frame new graphics");

        Timer::after(Duration::from_millis(1_000)).await;
    }
}