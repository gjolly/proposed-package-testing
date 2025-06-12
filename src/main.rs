// main.rs
use anyhow::{anyhow, Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// Helper function to execute a shell command and check its success.
/// Returns the command's stdout on success.
fn run_command(command: &str, args: &[&str], error_msg: &str) -> Result<String> {
    println!("Executing: {} {}", command, args.join(" "));
    let output = Command::new(command)
        .args(args)
        .output()
        .context(format!("Failed to execute command: {}", command))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow!(
            "{}: Command failed with status {:?}\nStdout: {}\nStderr: {}",
            error_msg,
            output.status.code(),
            stdout,
            stderr
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <image_url> <package_name>", args[0]);
        eprintln!("Example: sudo {} https://cloud-images.ubuntu.com/releases/jammy/release/ubuntu-22.04-server-cloudimg-amd64.img htop", args[0]);
        return Err(anyhow!("Missing arguments"));
    }

    let image_url = &args[1];
    let package_name = &args[2];

    println!("Starting VM image processing for URL: {}", image_url);
    println!("Package to install: {}", package_name);

    // Create a temporary directory for downloads and mounts
    let temp_base_dir = tempdir().context("Failed to create temporary base directory")?;
    let image_path = temp_base_dir.path().join("vm_image.img");
    let rootfs_dir = temp_base_dir.path().join("rootfs");
    let boot_dir = rootfs_dir.join("boot");
    let boot_efi_dir = boot_dir.join("efi");

    // Ensure cleanup happens even if errors occur
    let cleanup_guard = CleanupGuard {
        loop_device: None,
        rootfs_dir: rootfs_dir.clone(),
        temp_dir: temp_base_dir.path().to_path_buf(),
    };

    println!("Downloading VM Image");
    println!("Downloading {} to {:?}", image_url, image_path);
    let response = reqwest::get(image_url)
        .await
        .context(format!("Failed to fetch URL: {}", image_url))?
        .error_for_status()
        .context(format!("Bad status code from URL: {}", image_url))?;

    let mut dest = File::create(&image_path)
        .await
        .context("Failed to create image file")?;
    let content = response
        .bytes()
        .await
        .context("Failed to read response bytes")?;
    dest.write_all(&content)
        .await
        .context("Failed to write image content to file")?;
    println!("Image downloaded successfully to {:?}", image_path);

    // Use a block to ensure `cleanup_guard` is dropped at the end of `main`
    {
        // Mutate the cleanup_guard within the block
        let mut cleanup_guard = cleanup_guard; // Shadowing to make it mutable in this scope

        let raw_image_path = image_path.with_extension("raw");

        run_command(
            "qemu-img",
            &[
                "convert",
                "-f",
                "qcow2",
                "-O",
                "raw",
                image_path.to_str().unwrap(),
                raw_image_path.to_str().unwrap(),
            ],
            "Failed to attach loop device",
        )?;

        println!("Attaching image to loop device");
        let output = run_command(
            "losetup",
            &[
                "--partscan",
                "--show",
                "--find",
                raw_image_path.to_str().unwrap(),
            ],
            "Failed to attach loop device",
        )?;
        let loop_device_path = Some(output.clone());
        cleanup_guard.loop_device = loop_device_path.clone(); // Update guard
        println!("Image attached to loop device: {}", output);

        println!("Mounting partitions");
        // Create mount points
        fs::create_dir_all(&rootfs_dir).context("Failed to create rootfs directory")?;
        fs::create_dir_all(&boot_dir).context("Failed to create rootfs/boot directory")?;
        fs::create_dir_all(&boot_efi_dir).context("Failed to create rootfs/boot/efi directory")?;

        let loop_dev = loop_device_path.as_ref().unwrap();

        // Mount /dev/loopXp1 to rootfs
        run_command(
            "mount",
            &[
                format!("{loop_dev}p1").as_str(),
                rootfs_dir.to_str().unwrap(),
            ],
            "Failed to mount rootfs partition",
        )?;
        println!("Mounted {}p1 to {:?}", loop_dev, rootfs_dir);

        // Mount /dev/loopXp13 to rootfs/boot
        let mount_result = run_command(
            "mount",
            &[
                format!("{loop_dev}p13").as_str(),
                boot_dir.to_str().unwrap(),
            ],
            "Failed to mount boot partition",
        );

        if mount_result.is_err() {
            eprintln!(
                "Warning: Failed to mount boot partition {}p13: {}",
                loop_dev,
                mount_result.unwrap_err()
            );

            // Noble has it on p16 instead of p13
            // Mount /dev/loopXp16 to rootfs/boot
            // TOOD: check the release name instead of trying both
            let mount_result = run_command(
                "mount",
                &[
                    format!("{loop_dev}p16").as_str(),
                    boot_dir.to_str().unwrap(),
                ],
                "Failed to mount boot partition",
            );

            if mount_result.is_err() {
                eprintln!(
                    "Warning: Failed to mount boot partition {}p16: {}",
                    loop_dev,
                    mount_result.unwrap_err()
                );
            }
        }

        // Mount /dev/loopXp15 to rootfs/boot/efi
        run_command(
            "mount",
            &[
                format!("{loop_dev}p15").as_str(),
                boot_efi_dir.to_str().unwrap(),
            ],
            "Failed to mount boot/efi partition",
        )?;
        println!("Mounted {}p15 to {:?}", loop_dev, boot_efi_dir);

        println!("Modifying image with systemd-nspawn");
        println!("Enabling -proposed repository...");
        run_command(
            "systemd-nspawn",
            &[
                "-D",
                rootfs_dir.to_str().unwrap(),
                "apt-add-repository",
                "-y",
                "-proposed",
            ],
            "Failed to add proposed repository",
        )?;

        // Install the specified package
        println!("Installing package: {}...", package_name);
        run_command(
            "systemd-nspawn",
            &[
                "-D",
                rootfs_dir.to_str().unwrap(),
                "apt-get",
                "install",
                "-y", // Add -y for non-interactive
                package_name,
            ],
            &format!("Failed to install package {}", package_name),
        )?;
        println!("Package '{}' installed successfully.", package_name);

        println!("Disabling -proposed repository...");
        run_command(
            "systemd-nspawn",
            &[
                "-D",
                rootfs_dir.to_str().unwrap(),
                "apt-add-repository",
                "-y",
                "--remove",
                "-proposed",
            ],
            "Failed to add proposed repository",
        )?;

        // Extract the image name from the UR/L
        let image_name = image_url
            .split('/')
            .last()
            .unwrap()
            .trim_end_matches(".img");

        // Copy raw image to current directory
        // file format: {image_name}-{package_name}-proposed.raw
        let final_image_path =
            PathBuf::from(format!("{}_{}_proposed.raw", image_name, package_name));

        fs::copy(&raw_image_path, &final_image_path)
            .context("Failed to copy raw image to final destination")?;
    } // `cleanup_guard` is dropped here, triggering cleanup

    println!("All operations completed.");
    Ok(())
}

struct CleanupGuard {
    loop_device: Option<String>,
    rootfs_dir: PathBuf,
    temp_dir: PathBuf,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        println!("Cleaning up... ");

        if self.rootfs_dir.is_dir() {
            println!("Umounting rootfs directory: {:?}", self.rootfs_dir);
            let _ = run_command(
                "umount",
                &["-R", self.rootfs_dir.to_str().unwrap()],
                "Failed to unmount rootfs (during cleanup)",
            )
            .unwrap();
        }

        // Detach loop device
        if let Some(ref dev) = self.loop_device {
            let _ = run_command(
                "losetup",
                &["-d", dev],
                "Failed to detach loop device (during cleanup)",
            )
            .unwrap();
        }

        // Remove temporary directories and image file
        if self.temp_dir.exists() {
            let _ = fs::remove_dir_all(&self.rootfs_dir)
                .context("Failed to remove rootfs directory")
                .unwrap();
        }
        println!("Cleanup complete.");
    }
}
