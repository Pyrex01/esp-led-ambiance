#!/usr/bin/env sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 ESP32_IP" >&2
    exit 2
fi

device_ip=$1
elf=target/xtensa-esp32s3-none-elf/release/practice-esp
image=target/ota-firmware.bin

cargo build --release
espflash save-image --chip esp32s3 --partition-table partitions.csv \
    --target-app-partition ota_0 "$elf" "$image"
curl --fail --show-error --progress-bar --max-time 180 \
    --header 'Content-Type: application/octet-stream' \
    --data-binary "@$image" "http://$device_ip/update"
