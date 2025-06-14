# propose-package-testing

Download an Ubuntu cloud-image and customize it to installs a package from the proposed pocket.

## Build

```bash
cargo build --release
```

## Usage

Pre-requisites:

```bash
sudo apt install -y qemu-utils
sudo modprobe nbd
```

Run the tool with a remote image:

```bash
sudo ./target/release/proposed_package_testing https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-amd64.img walinuxagent
```

or with a local file:

```bash
sudo ./target/release/proposed_package_testing ./ubuntu-24.04-server-cloudimg-amd64.img walinuxagent
```

This will produce two files:
 * a new QCOW2 images
 * a `metadata.yaml` required to import the image in LXD.

## Use image with LXD

Import the image:

```bash
tar --transform 'flags=r;s/.*.img/rootfs.img/' -czf image.tar.gz ./metadata.yaml ./ubuntu-24.04-server-cloudimg-amd64_walinuxagent_proposed.img
lxc image import --alias ubuntu-proposed-testing ./image.tar.gz
```

Start a VM with this image:

```bash
lxc launch --vm ubuntu-proposed-testing noble-walinuxagent
```

## Limitations

 * Updating the booloader or any of the boot assets is currently not supported as `update-grub` will not work.
 * Installing Snaps is currently not supported.
 * Only amd64 is currently supported.
 * Only QCOW2 images are supported for now.
