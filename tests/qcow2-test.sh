#!/bin/bash -eu

PACKAGE="btop"
IMAGE="ubuntu-24.04-server-cloudimg-amd64"
IMAGE_URL="https://cloud-images.ubuntu.com/releases/noble/release/$IMAGE.img"

if [ ! -f "$IMAGE.img" ]; then
    echo "Downloading image $IMAGE.img from $IMAGE_URL"
    curl -LO "$IMAGE_URL"
else
    echo "Image $IMAGE already exists, skipping download"
fi

echo "Running integration tests with local image $IMAGE.img"
./target/debug/proposed_package_testing "$IMAGE.img" "$PACKAGE"

expected_output_image="${IMAGE}_${PACKAGE}.img"
if [ ! -f "$expected_output_image" ]; then
    echo "Integration tests failed: ${expected_output_image} image not found"
    exit 1
fi

qemu-nbd --connect /dev/nbd0 "$expected_output_image"
sleep 2

rootfs="$(mktemp -d)"
mount /dev/nbd0p1 "$rootfs"

chroot "$rootfs" apt list --installed 2> /dev/null | grep -q "$PACKAGE" || exit 1

umount "$rootfs"
qemu-nbd --disconnect /dev/nbd0
rm -rf "$rootfs"

rm "$expected_output_image"
echo "QCOW2 test completed successfully"
