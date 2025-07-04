#!/bin/bash

# Usage: 
#  ./upload-image-to-azure.sh <resource_group> <location> <gallery_name> <vhd_path> [image_version]
# This script creates an image gallery in the specified resource group, uploads the disk
# image to Azure and register the image in the gallery.

set -euo pipefail

if [ "$#" -ne 4 ] && [ "$#" -ne 5 ]; then
  echo "Usage: $0 <resource_group> <location> <gallery_name> <vhd_path> [image_version]"
  exit 1
fi

RESOURCE_GROUP="$1"
LOCATION="$2"
GALLERY_NAME="$3"
VHD_PATH="$4"

# Constants / derived values
TEMP_DISK_NAME="gjolly-gallery-$(date +%s)"
IMAGE_DEF_NAME="imageDef"
IMAGE_VERSION="${5:-"1.0.0"}"
DISK_SIZE=$(stat --format '%s' "${VHD_PATH}")

OS_TYPE="Linux"
PUBLISHER="gjolly"
OFFER="ubuntu"
SKU="test"
HYPERV_GEN="V2"

# Create the gallery if it doesn't exist
if ! az sig show --resource-group "$RESOURCE_GROUP" --gallery-name "$GALLERY_NAME" &>/dev/null; then
  echo "==> Creating Shared Image Gallery '$GALLERY_NAME'..."
  az sig create \
    --resource-group "$RESOURCE_GROUP" \
    --gallery-name "$GALLERY_NAME" \
    --location "$LOCATION"
else
  echo "==> Shared Image Gallery '$GALLERY_NAME' already exists."
fi

# Create image definition if it doesn't exist
if ! az sig image-definition show \
  --resource-group "$RESOURCE_GROUP" \
  --gallery-name "$GALLERY_NAME" \
  --gallery-image-definition "$IMAGE_DEF_NAME" &>/dev/null; then
  echo "==> Creating image definition '$IMAGE_DEF_NAME'..."
  az sig image-definition create \
    --resource-group "$RESOURCE_GROUP" \
    --gallery-name "$GALLERY_NAME" \
    --gallery-image-definition "$IMAGE_DEF_NAME" \
    --publisher "$PUBLISHER" \
    --offer "$OFFER" \
    --sku "$SKU" \
    --os-type "$OS_TYPE" \
    --hyper-v-generation "$HYPERV_GEN" \
    --location "$LOCATION"
else
  echo "==> Image definition '$IMAGE_DEF_NAME' already exists."
fi

# Create managed image with upload type
echo "==> Creating temporary managed image '$TEMP_DISK_NAME'..."
DISK_ID="$(az disk create \
  --resource-group "$RESOURCE_GROUP" \
  --name "$TEMP_DISK_NAME" \
  --os-type "$OS_TYPE" \
  --hyper-v-generation "$HYPERV_GEN" \
  --location "$LOCATION" \
  --upload-type Upload \
  --sku standard_lrs \
  --upload-size-bytes "$DISK_SIZE" | jq -r ".id")"

echo "==> Generating SAS URL..."
SAS_URL=$(
    az disk grant-access \
      --name "$TEMP_DISK_NAME" \
      --resource-group "$RESOURCE_GROUP" \
      --access-level Write \
      --duration-in-seconds 86400 | jq --raw-output .accessSAS)

echo "==> Uploading VHD to managed image using AzCopy..."
azcopy copy "$VHD_PATH" "$SAS_URL" --blob-type PageBlob

az disk revoke-access \
  --name "$TEMP_DISK_NAME" \
  --resource-group "$RESOURCE_GROUP"

echo "==> Waiting for managed image provisioning to complete..."
while true; do
  STATE=$(az disk show \
    --resource-group "$RESOURCE_GROUP" \
    --name "$TEMP_DISK_NAME" \
    --query "provisioningState" -o tsv)
  if [ "$STATE" == "Succeeded" ]; then
    echo "✅ Upload complete."
    break
  fi
  echo "⏳ Current provisioning state: $STATE"
  sleep 10
done

# Publish to gallery
echo "==> Creating image version '$IMAGE_VERSION' in gallery..."
IMAGE_ID=$(az sig image-version create \
  --resource-group "$RESOURCE_GROUP" \
  --gallery-name "$GALLERY_NAME" \
  --gallery-image-definition "$IMAGE_DEF_NAME" \
  --gallery-image-version "$IMAGE_VERSION" \
  --location "$LOCATION" \
  --os-snapshot "$DISK_ID" \
  --replica-count 1 | jq -r '.id')

echo "✅ Image uploaded to gallery and managed image cleaned up."
echo "   Image ID:"
echo "   $IMAGE_ID"

# Clean up
echo "==> Deleting temporary managed image '$TEMP_DISK_NAME'..."
az disk delete --yes --no-wait \
  --resource-group "$RESOURCE_GROUP" \
  --name "$TEMP_DISK_NAME"
