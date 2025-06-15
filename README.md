# propose-package-testing

Download an Ubuntu cloud-image and customize it to installs a given package.

## Install

Download the pre-built binary from the latest release: https://github.com/gjolly/proposed-package-testing/releases/latest

You can also install from source using `cargo`:

```bash
cargo install --git https://github.com/gjolly/proposed-package-testing
```

## Usage

Pre-requisites:

```bash
sudo apt install -y qemu-utils systemd-container
```

Run the tool with a remote image:

```bash
sudo proposed_package_testing https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-amd64.img walinuxagent
```

or with a local file:

```bash
sudo proposed_package_testing ./ubuntu-24.04-server-cloudimg-amd64.img walinuxagent
```

Features:
 * install a package from the proposed pocket, use `--proposed`
 * build a ready-to-import LXD tarball with `--lxd`

## Use the image with LXD

Import the image:

```bash
lxc image import --alias ubuntu-proposed-testing ./ubuntu-24.04-server-cloudimg-amd64_walinuxagent_proposed.tar.gz
```

Start a VM with this image:

```bash
lxc launch --vm ubuntu-proposed-testing noble-walinuxagent
```

## Limitations

 * Updating the booloader or any of the boot assets is currently not supported as `update-grub` will not work.
 * Installing Snaps is currently not supported.
 * Only amd64 is currently supported.
 * Only QCOW2 images and LXD tarballs are supported for now.
