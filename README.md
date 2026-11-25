Toy project on Waveshare ESP32-C6-Touch-AMOLED-2.16

# Developer guide
1. Rustup is required on your machine. See https://rust-lang.org/tools/install/
1. Install the required Rust target using rustup: `rustup target add riscv32imac-unknown-none-elf`.
1. Install the ESP flashing tool: `cargo install esp-generate espflash`.
1. Plug in the board via it's USB-C port and run: `cargo run --release`.
This compiles the code, flashes it on the board, and monitors the logs using the serial interface.

