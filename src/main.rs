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

async fn download_image(url: &str, dest: &PathBuf) -> Result<()> {
    let response = reqwest::get(url)
        .await
        .context(format!("Failed to fetch URL: {}", url))?
        .error_for_status()
        .context(format!("Bad status code from URL: {}", url))?;

    let mut dest = File::create(&dest)
        .await
        .context("Failed to create image file")?;
    let content = response
        .bytes()
        .await
        .context("Failed to read response bytes")?;
    dest.write_all(&content)
        .await
        .context("Failed to write image content to file")?;

    Ok(())
}

fn connect_image_to_nbd(image_path: &PathBuf, nbd_device_path: &str) -> Result<()> {
    // Ensure the nbd kernel module is loaded
    run_command("modprobe", &["nbd"], "Failed to load nbd kernel module")
        .context("Failed to load nbd kernel module")?;

    // Connect the image to the NBD device
    run_command(
        "qemu-nbd",
        &["-c", nbd_device_path, image_path.to_str().unwrap()],
        "Failed to connect image to NBD device",
    )
    .context("Failed to connect image to NBD device")?;

    // Sleep for a short duration to ensure the device is ready
    std::thread::sleep(std::time::Duration::from_secs(2));

    Ok(())
}

fn mount_partition(device: &str, mount_point: &PathBuf) -> Result<()> {
    fs::create_dir_all(mount_point).context("Failed to create mount point directory")?;

    run_command(
        "mount",
        &[device, mount_point.to_str().unwrap()],
        &format!("Failed to mount {} to {}", device, mount_point.display()),
    )?;

    Ok(())
}

fn configure_dns(rootfs_dir: &PathBuf) -> Result<()> {
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
    Ok(())
}

fn restore_dns(rootfs_dir: &PathBuf) -> Result<()> {
    let resolv_conf_path = rootfs_dir.join("etc/resolv.conf");
    let resolv_conf_backup_path = rootfs_dir.join("etc/resolv.conf.bak");
    if resolv_conf_backup_path.exists() {
        println!("Restoring original resolv.conf from backup.");
        fs::rename(&resolv_conf_backup_path, &resolv_conf_path)
            .context("Failed to restore resolv.conf from backup")?;
    } else {
        println!("No backup found for resolv.conf, skipping restore.");
    }
    Ok(())
}

fn enable_proposed_repository(rootfs_dir: &PathBuf) -> Result<()> {
    let apt_add_repo_args = vec![
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
    Ok(())
}

fn disable_proposed_repository(rootfs_dir: &PathBuf) -> Result<()> {
    let apt_add_repo_args = vec![
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
        "--remove",
    ];

    run_command(
        "systemd-nspawn",
        &apt_add_repo_args,
        "Failed to remove proposed repository",
    )?;

    Ok(())
}

fn get_release(rootfs_dir: &PathBuf) -> Result<String> {
    let os_release_content = fs::read_to_string(rootfs_dir.join("etc/os-release"))
        .context("Failed to read /etc/os-release")?;
    let release = os_release_content
        .lines()
        .find(|line| line.starts_with("VERSION_CODENAME="))
        .and_then(|line| line.split('=').nth(1))
        .map(|s| s.trim_matches('"').to_lowercase())
        .ok_or_else(|| anyhow!("Failed to determine release name from /etc/os-release"))?;

    Ok(release)
}

fn install_package(rootfs_dir: &PathBuf, package_name: &str) -> Result<()> {
    let release = get_release(rootfs_dir)?;
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
    Ok(())
}

fn generate_lxd_metadata(package_name: &str, release: &str) -> Result<()> {
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

    fs::write("metadata.yaml", lxd_metadata).context("Failed to write LXD metadata")
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

    // Extract the image name from the UR/L
    let image_name = image_url
        .split('/')
        .last()
        .unwrap()
        .trim_end_matches(".img");

    // Copy image to current directory
    // file format: {image_name}-{package_name}-proposed.img
    let final_image_path = PathBuf::from(format!("{}_{}_proposed.img", image_name, package_name));

    // Determine if image_url is a URL or a local file path
    if image_url.starts_with("http://") || image_url.starts_with("https://") {
        // Treat as local file path, copy to image_path
        println!("Using local image file: {}", image_url);
        fs::copy(image_url, &image_path)
            .with_context(|| format!("Failed to copy local image file from {}", image_url))?;
    } else {
        // Download the image from the URL
        println!("Downloading image from URL: {}", image_url);
        download_image(image_url, &image_path).await?;
    }

    let release: String;

    // Use a block to ensure `cleanup_guard` is dropped at the end of `main`
    {
        // Mutate the cleanup_guard within the block
        let mut cleanup_guard = cleanup_guard;
        let nbd_device_path = "/dev/nbd0";

        connect_image_to_nbd(&image_path, nbd_device_path)?;

        println!("Attaching image to loop device using qemu-nbd");
        connect_image_to_nbd(&image_path, nbd_device_path)?;

        cleanup_guard.nbd_device_path = Some(nbd_device_path.to_string());

        println!("Mounting partitions");
        // Create mount points
        fs::create_dir_all(&rootfs_dir).context("Failed to create rootfs directory")?;

        // Mount /dev/loopXp1 to rootfs
        mount_partition(&format!("{}p1", nbd_device_path), &rootfs_dir)?;

        // Mount /dev/loopXp13 to rootfs/boot
        let mount_result = mount_partition(&format!("{}p13", nbd_device_path), &boot_dir);

        if mount_result.is_err() {
            // Noble has it on p16 instead of p13
            // Mount /dev/loopXp16 to rootfs/boot
            let _ = mount_partition(&format!("{}p13", nbd_device_path), &boot_dir);
        }

        // Mount /dev/loopXp15 to rootfs/boot/efi
        mount_partition(&format!("{}p15", nbd_device_path), &boot_efi_dir)?;

        println!("Configuring DNS settings");
        configure_dns(&rootfs_dir)?;

        println!("Modifying image with systemd-nspawn");
        println!("Enabling -proposed repository...");
        enable_proposed_repository(&rootfs_dir)?;

        // Determine the release name from the image URL
        release = get_release(&rootfs_dir)?;

        // Install the specified package
        println!("Installing package: {}...", package_name);
        install_package(&rootfs_dir, package_name)?;
        println!(
            "Package '{}' installed successfully from proposed.",
            package_name
        );

        println!("Disabling -proposed repository...");
        disable_proposed_repository(&rootfs_dir)?;

        restore_dns(&rootfs_dir)?;
    } // `cleanup_guard` is dropped here, triggering cleanup

    fs::copy(&image_path, &final_image_path)
        .context("Failed to copy image to current directory")?;
    // Write lxd metadata
    generate_lxd_metadata(package_name, &release)?;
    println!("Custom image created: {}", final_image_path.display());
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
