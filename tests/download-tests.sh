#!/bin/bash -eu

PACKAGE="btop"
IMAGE="ubuntu-24.04-server-cloudimg-amd64"
IMAGE_URL="https://cloud-images.ubuntu.com/releases/noble/release/$IMAGE.img"

source "/etc/os-release"

if [ "$ID" == "ubuntu" ]; then
    echo "Installing dependencies"
    apt-get update
    apt-get install -y qemu-system-x86 qemu-utils
fi

echo "Running integration tests with image $IMAGE_URL"
./target/debug/proposed_package_testing "$IMAGE_URL" "$PACKAGE"

expected_output_image="${IMAGE}_${PACKAGE}.img"
if [ ! -f "$expected_output_image" ]; then
    echo "Integration tests failed: ${expected_output_image} image not found"
    exit 1
fi

if [ ! -f "metadata.yaml" ]; then
    echo "Integration tests failed: metadata.yaml not found"
    exit 1
fi

echo "Download test completed successfully"
