#!/usr/bin/env sh
# Build the firmware for the Wokwi simulator and produce a merged flash image.
set -eu

elf=target/xtensa-esp32s3-none-elf/release/practice-esp

cargo build --release --features wokwi
espflash save-image --chip esp32s3 --merge --partition-table partitions.csv \
    "$elf" target/wokwi-flash.bin
