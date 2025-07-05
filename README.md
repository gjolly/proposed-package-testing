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
sudo proposed_package_testing \
  https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-amd64.img \
  walinuxagent
```

or with a local file:

```bash
sudo proposed_package_testing ./ubuntu-24.04-server-cloudimg-amd64.img walinuxagent
```

Features:
 * install a package from the proposed pocket, use `--proposed`
 * install a package from a PPA with `--ppa ppa:owner/name`
 * build a ready-to-import LXD tarball with `--lxd`

## Use the image with LXD

Import the image:

```bash
lxc image import \
  --alias ubuntu-proposed-testing \
  ./ubuntu-24.04-server-cloudimg-amd64_walinuxagent_proposed.tar.gz
```

Start a VM with this image:

```bash
lxc launch --vm ubuntu-proposed-testing noble-walinuxagent
```

## Using Azure

### Customize the image

Download the image from `cloud-images.ubuntu.com`:

```bash
curl -LO https://cloud-images.ubuntu.com/questing/current/questing-server-cloudimg-amd64-azure.vhd.tar.gz
tar xvSf questing-server-cloudimg-amd64-azure.vhd.tar.gz
```

Use the tool to customize the image (use the `vpc` format):

```bash
sudo proposed_package_testing \
  --image-format vpc \
  /tmp/livecd.ubuntu-cpc.azure.vhd \
  btop
```

### Upload to an image gallery

With the Azure CLI installed and configured:

```bash
az group create -l 'westeurope' --name mygroup
./scripts/upload-image-to-azure.sh \
  mygroup westeurope testgallery \
  ./image.vhd '25.10.202507040'
```

Then you can create a VM from the image:

```bash
az vm create \
  --resource-group mygroup \
  --name questing-btop \
  --image "<image_id>" \
  --ssh-key-values /path/to/ssh/key \
  --size Standard_B2als_v2 \
  --admin-username ubuntu
```

## Limitations

 * Updating the booloader or any of the boot assets is currently not supported as `update-grub` will not work.
 * Installing Snaps is currently not supported.
 * Only amd64 is currently supported.
 * Only QCOW2 images and LXD tarballs are supported for now.
