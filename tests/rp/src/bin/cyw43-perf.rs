#![no_std]
#![no_main]
teleprobe_meta::target!(b"rpi-pico");

use cyw43::JoinOptions;
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use defmt::{panic, *};
use embassy_executor::Spawner;
use embassy_net::{Config, StackResources};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::{bind_interrupts, rom_data};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

teleprobe_meta::timeout!(120);

// Test-only wifi network, no internet access!
const WIFI_NETWORK: &str = "EmbassyTestWPA2";
const WIFI_PASSWORD: &str = "V8YxhKt5CdIAJFud";

#[embassy_executor::task]
async fn wifi_task(runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Hello World!");
    let p = embassy_rp::init(Default::default());

    // needed for reading the firmware from flash via XIP.
    unsafe {
        rom_data::flash_exit_xip();
        rom_data::flash_enter_cmd_xip();
    }

    // cyw43 firmware needs to be flashed manually:
    //     probe-rs download 43439A0.bin --binary-format bin --chip RP2040 --base-address 0x101b0000
    //     probe-rs download 43439A0_btfw.bin --binary-format bin --chip RP2040 --base-address 0x101f0000
    //     probe-rs download 43439A0_clm.bin --binary-format bin --chip RP2040 --base-address 0x101f8000
    let fw = unsafe { core::slice::from_raw_parts(0x101b0000 as *const u8, 231077) };
    let _btfw = unsafe { core::slice::from_raw_parts(0x101f0000 as *const u8, 6164) };
    let clm = unsafe { core::slice::from_raw_parts(0x101f8000 as *const u8, 984) };

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(wifi_task(runner)));

    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // Generate random seed
    let seed = 0x0123_4567_89ab_cdef; // chosen by fair dice roll. guarenteed to be random.

    // Init network stack
    static RESOURCES: StaticCell<StackResources<2>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(
        net_device,
        Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    unwrap!(spawner.spawn(net_task(runner)));

    loop {
        match control
            .join(WIFI_NETWORK, JoinOptions::new(WIFI_PASSWORD.as_bytes()))
            .await
        {
            Ok(_) => break,
            Err(err) => {
                panic!("join failed with status={}", err.status);
            }
        }
    }

    perf_client::run(
        stack,
        perf_client::Expected {
            down_kbps: 200,
            up_kbps: 200,
            updown_kbps: 200,
        },
    )
    .await;

    info!("Test OK");
    cortex_m::asm::bkpt();
}
