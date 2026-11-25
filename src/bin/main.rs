#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Address, Command, Config as SpiConfig, DataMode, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

const AXP2101_ADDR: u8 = 0x34;
const WIDTH: u16 = 480;
const HEIGHT: u16 = 480;

/// Send a 1-line QSPI command + optional parameters to the CO5300
fn co5300_cmd(spi: &mut Spi<'_, esp_hal::Blocking>, cmd: u8, data: &[u8]) {
    spi.half_duplex_write(
        DataMode::Single,
        Command::_8Bit(0x02, DataMode::Single),
        Address::_24Bit((cmd as u32) << 8, DataMode::Single),
        0,
        data,
    )
    .unwrap();
}

/// Fill the entire 480x480 screen with a single RGB565 color
fn fill_screen(spi: &mut Spi<'_, esp_hal::Blocking>, rgb565: u16) {
    // Column address: 0..479
    let x_end = (WIDTH - 1).to_be_bytes();
    co5300_cmd(spi, 0x2A, &[0x00, 0x00, x_end[0], x_end[1]]);

    // Row address: 0..479
    let y_end = (HEIGHT - 1).to_be_bytes();
    co5300_cmd(spi, 0x2B, &[0x00, 0x00, y_end[0], y_end[1]]);

    // Blocking SPI FIFO is 64 bytes (32 RGB565 pixels) per write
    let [hi, lo] = rgb565.to_be_bytes();
    let mut chunk = [0u8; 64];
    for px in chunk.chunks_exact_mut(2) {
        px[0] = hi;
        px[1] = lo;
    }

    let total_chunks = (WIDTH as usize * HEIGHT as usize) / 32;
    for i in 0..total_chunks {
        // 0x2C = start RAM write, 0x3C = continue RAM write
        let ram_cmd: u32 = if i == 0 { 0x2C } else { 0x3C };
        spi.half_duplex_write(
            DataMode::Quad,
            Command::_8Bit(0x32, DataMode::Single),
            Address::_24Bit(ram_cmd << 8, DataMode::Single),
            0,
            &chunk,
        )
        .unwrap();
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("Embassy initialized!");

    // 1. Power-cycle / reset the display via AXP2101 ALDO3 (3.3V)
    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO8)
    .with_scl(peripherals.GPIO7);

    // Reg 0x94 = ALDO3 voltage: (3300mV - 500mV) / 100mV = 28 (0x1C)
    i2c.write(AXP2101_ADDR, &[0x94, 0x1C]).unwrap();

    // Reg 0x90 bit 2 = ALDO3 enable. Toggle off -> on to reset CO5300.
    let mut ldo_en = [0u8; 1];
    i2c.write_read(AXP2101_ADDR, &[0x90], &mut ldo_en).unwrap();
    i2c.write(AXP2101_ADDR, &[0x90, ldo_en[0] & !0x04]).unwrap();
    Timer::after(Duration::from_millis(20)).await;
    i2c.write(AXP2101_ADDR, &[0x90, ldo_en[0] | 0x04]).unwrap();
    Timer::after(Duration::from_millis(150)).await;

    // 2. Configure QSPI on SPI2 for the CO5300 AMOLED
    let mut spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(40))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO0)
    .with_cs(peripherals.GPIO15)
    .with_sio0(peripherals.GPIO1)
    .with_sio1(peripherals.GPIO2)
    .with_sio2(peripherals.GPIO3)
    .with_sio3(peripherals.GPIO4);

    // 3. Minimal CO5300 init sequence
    co5300_cmd(&mut spi, 0x11, &[]); // Sleep Out
    Timer::after(Duration::from_millis(120)).await;
    co5300_cmd(&mut spi, 0xFE, &[0x00]); // User command set
    co5300_cmd(&mut spi, 0xC4, &[0x80]); // Enable QSPI mode
    co5300_cmd(&mut spi, 0x3A, &[0x55]); // 16-bit RGB565
    co5300_cmd(&mut spi, 0x35, &[0x00]); // Tearing effect on
    co5300_cmd(&mut spi, 0x53, &[0x20]); // Enable brightness control
    co5300_cmd(&mut spi, 0x51, &[0xFF]); // Max brightness
    co5300_cmd(&mut spi, 0x63, &[0xFF]); // Max HBM brightness
    co5300_cmd(&mut spi, 0x29, &[]); // Display On
    Timer::after(Duration::from_millis(20)).await;

    info!("AMOLED initialized!");

    let _ = spawner;

    // Cycle Red, Green, Blue, Black (RGB565)
    let colors: [(&str, u16); 4] = [
        ("Red", 0xF800),
        ("Green", 0x07E0),
        ("Blue", 0x001F),
        ("Black", 0x0000),
    ];

    loop {
        for (name, color) in colors {
            info!("Filling screen: {}", name);
            fill_screen(&mut spi, color);
            Timer::after(Duration::from_secs(1)).await;
        }
    }
}
