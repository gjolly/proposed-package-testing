// main.rs
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
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
        nbd_device_path: None,
        rootfs_dir: rootfs_dir.clone(),
    };

    // Determine if image_url is a URL or a local file path
    let is_url = image_url.starts_with("http://") || image_url.starts_with("https://");
    if !is_url {
        // Treat as local file path, copy to image_path
        println!("Using local image file: {}", image_url);
        fs::copy(image_url, &image_path)
            .with_context(|| format!("Failed to copy local image file from {}", image_url))?;
    } else {
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
    }

    // Use a block to ensure `cleanup_guard` is dropped at the end of `main`
    {
        // Mutate the cleanup_guard within the block
        let mut cleanup_guard = cleanup_guard; // Shadowing to make it mutable in this scope

        // Use qemu-nbd to attach the image
        let nbd_device_path = "/dev/nbd0";

        println!("Attaching image to loop device using qemu-nbd");
        let output = run_command(
            "qemu-nbd",
            &["-c", "/dev/nbd0", image_path.to_str().unwrap()],
            "Failed to attach image to loop device (verify nbd kernel module is loaded)",
        )?;

        // Sleep for a short duration to ensure the device is ready
        std::thread::sleep(std::time::Duration::from_secs(2));

        cleanup_guard.nbd_device_path = Some(nbd_device_path.to_string());
        println!("Image attached to loop device: {}", output);

        println!("Mounting partitions");
        // Create mount points
        fs::create_dir_all(&rootfs_dir).context("Failed to create rootfs directory")?;
        fs::create_dir_all(&boot_dir).context("Failed to create rootfs/boot directory")?;
        fs::create_dir_all(&boot_efi_dir).context("Failed to create rootfs/boot/efi directory")?;

        // Mount /dev/loopXp1 to rootfs
        run_command(
            "mount",
            &[
                format!("{nbd_device_path}p1").as_str(),
                rootfs_dir.to_str().unwrap(),
            ],
            "Failed to mount rootfs partition",
        )?;
        println!("Mounted {}p1 to {:?}", nbd_device_path, rootfs_dir);

        // Mount /dev/loopXp13 to rootfs/boot
        let mount_result = run_command(
            "mount",
            &[
                format!("{nbd_device_path}p13").as_str(),
                boot_dir.to_str().unwrap(),
            ],
            "Failed to mount boot partition",
        );

        if mount_result.is_err() {
            eprintln!(
                "Warning: Failed to mount boot partition {}p13: {}",
                nbd_device_path,
                mount_result.unwrap_err()
            );

            // Noble has it on p16 instead of p13
            // Mount /dev/loopXp16 to rootfs/boot
            // TOOD: check the release name instead of trying both
            let mount_result = run_command(
                "mount",
                &[
                    format!("{nbd_device_path}p16").as_str(),
                    boot_dir.to_str().unwrap(),
                ],
                "Failed to mount boot partition",
            );

            if mount_result.is_err() {
                eprintln!(
                    "Warning: Failed to mount boot partition {}p16: {}",
                    nbd_device_path,
                    mount_result.unwrap_err()
                );
            }
        }

        // Mount /dev/loopXp15 to rootfs/boot/efi
        run_command(
            "mount",
            &[
                format!("{nbd_device_path}p15").as_str(),
                boot_efi_dir.to_str().unwrap(),
            ],
            "Failed to mount boot/efi partition",
        )?;
        println!("Mounted {}p15 to {:?}", nbd_device_path, boot_efi_dir);

        println!("Configuring DNS settings");
        // Backup the original resolv.conf if it exists
        let resolv_conf_path = rootfs_dir.join("etc/resolv.conf");
        let resolv_conf_backup_path = rootfs_dir.join("etc/resolv.conf.bak");
        if resolv_conf_path.is_symlink() || resolv_conf_path.exists() {
            println!("Backing up original resolv.conf to resolv.conf.bak");
            fs::rename(&resolv_conf_path, &resolv_conf_backup_path)
                .context("Failed to backup resolv.conf")?;
        }

        // Write a custom resolv.conf with only "nameserver 1.1.1.1"
        fs::write(&resolv_conf_path, "nameserver 1.1.1.1\n")
            .context("Failed to write custom resolv.conf")?;

        println!("Modifying image with systemd-nspawn");
        println!("Enabling -proposed repository...");
        let mut apt_add_repo_args = vec![
            "-D",
            rootfs_dir.to_str().unwrap(),
            "apt-add-repository",
            "--yes",
            "--uri",
            "http://archive.ubuntu.com/ubuntu/",
            "--pocket",
            "proposed",
            "--component",
            "main",
            "--component",
            "universe",
        ];
        run_command(
            "systemd-nspawn",
            &apt_add_repo_args,
            "Failed to add proposed repository",
        )?;

        // Determine the release name from the image URL
        let os_release_content = fs::read_to_string(&rootfs_dir.join("etc/os-release"))
            .context("Failed to read /etc/os-release")?;
        let release = os_release_content
            .lines()
            .find(|line| line.starts_with("VERSION_CODENAME="))
            .and_then(|line| line.split('=').nth(1))
            .map(|s| s.trim_matches('"').to_lowercase())
            .ok_or_else(|| anyhow!("Failed to determine release name from /etc/os-release"))?;

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
                &format!("{}/{}-proposed", package_name, release),
            ],
            &format!("Failed to install package {}", package_name),
        )?;
        println!(
            "Package '{}' installed successfully from proposed.",
            package_name
        );

        println!("Disabling -proposed repository...");
        apt_add_repo_args.push("--remove");
        run_command(
            "systemd-nspawn",
            &apt_add_repo_args,
            "Failed to add proposed repository",
        )?;

        // Restore original resolv.conf if backup exists
        let resolv_conf_path = rootfs_dir.join("etc/resolv.conf");
        let resolv_conf_backup_path = rootfs_dir.join("etc/resolv.conf.bak");
        if resolv_conf_backup_path.exists() {
            println!("Restoring original resolv.conf from backup.");
            let _ = std::fs::rename(&resolv_conf_backup_path, &resolv_conf_path);
        }

        // Extract the image name from the UR/L
        let image_name = image_url
            .split('/')
            .last()
            .unwrap()
            .trim_end_matches(".img");

        // Copy image to current directory
        // file format: {image_name}-{package_name}-proposed.img
        let final_image_path =
            PathBuf::from(format!("{}_{}_proposed.img", image_name, package_name));

        fs::copy(&image_path, &final_image_path)
            .context("Failed to copy image to current directory")?;

        // Write lxd metadata
        let lxd_metadata = format!(
            r#"architecture: x86_64
creation_date: {}
properties:
  description: "Ubuntu {} with {} from proposed"
  os: Ubuntu
  release: "{}"
"#,
            Utc::now().timestamp(),
            release,
            package_name,
            release
        );

        fs::write("metadata.yaml", lxd_metadata)?;
    } // `cleanup_guard` is dropped here, triggering cleanup

    println!("All operations completed.");
    Ok(())
}

struct CleanupGuard {
    nbd_device_path: Option<String>,
    rootfs_dir: PathBuf,
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
            );
        }

        // Detach loop device
        if let Some(ref dev) = self.nbd_device_path {
            let _ = run_command(
                "qemu-nbd",
                &["--disconnect", dev],
                "Failed to disconnect nbd device (during cleanup)",
            )
            .unwrap();
        }

        println!("Cleanup complete.");
    }
}
