#!/usr/bin/env sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 ESP32_IP" >&2
    exit 2
fi

device_ip=$1
elf=target/xtensa-esp32s3-none-elf/release/practice-esp
image=target/ota-firmware.bin

# The Xtensa linker comes from espup's environment script; load it if this
# shell hasn't already.
if ! command -v xtensa-esp32s3-elf-gcc >/dev/null 2>&1 && [ -f "$HOME/export-esp.sh" ]; then
    . "$HOME/export-esp.sh"
fi

cargo build --release
espflash save-image --chip esp32s3 --partition-table partitions.csv \
    --target-app-partition ota_0 "$elf" "$image"
curl --fail --show-error --progress-bar --max-time 180 \
    --header 'Content-Type: application/octet-stream' \
    --data-binary "@$image" "http://$device_ip/update"
