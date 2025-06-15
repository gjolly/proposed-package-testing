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

echo "Running integration tests with image local $IMAGE.img"
./target/debug/proposed_package_testing --lxd "$IMAGE.img" "$PACKAGE"

expected_output_image="${IMAGE}_${PACKAGE}.tar.gz"
if [ ! -f "$expected_output_image" ]; then
    echo "Integration tests failed: ${expected_output_image} image not found"
    exit 1
fi

tar -tf "$expected_output_image" | grep -q "metadata.yaml" || exit 1
tar -tf "$expected_output_image" | grep -q "rootfs.img" || exit 1

rm "$expected_output_image"
echo "LXD test completed successfully"
