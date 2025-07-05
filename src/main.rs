// main.rs
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::Parser;
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

fn connect_image_to_nbd(image_path: &PathBuf, format: &str, nbd_device_path: &str) -> Result<()> {
    // Ensure the nbd kernel module is loaded
    run_command("modprobe", &["nbd"], "Failed to load nbd kernel module")
        .context("Failed to load nbd kernel module")?;

    // Connect the image to the NBD device
    run_command(
        "qemu-nbd",
        &[
            "--format",
            format,
            "--connect",
            nbd_device_path,
            image_path.to_str().unwrap(),
        ],
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
    if resolv_conf_backup_path.exists() || resolv_conf_backup_path.is_symlink() {
        println!("Restoring original resolv.conf from backup.");
        fs::rename(&resolv_conf_backup_path, &resolv_conf_path)
            .context("Failed to restore resolv.conf from backup")?;
    } else {
        println!("No backup found for resolv.conf, skipping restore.");
    }
    Ok(())
}

fn add_ppa(rootfs_dir: &PathBuf, ppa: &str) -> Result<()> {
    let apt_add_repo_args = vec![
        "-D",
        rootfs_dir.to_str().unwrap(),
        "apt-add-repository",
        "--no-update",
        "--yes",
        ppa,
    ];

    run_command(
        "systemd-nspawn",
        &apt_add_repo_args,
        "Failed to add proposed repository",
    )?;
    Ok(())
}

fn remove_ppa(rootfs_dir: &PathBuf, ppa: &str) -> Result<()> {
    let apt_add_repo_args = vec![
        "-D",
        rootfs_dir.to_str().unwrap(),
        "apt-add-repository",
        "--yes",
        "--remove",
        ppa,
    ];

    run_command(
        "systemd-nspawn",
        &apt_add_repo_args,
        "Failed to add proposed repository",
    )?;
    Ok(())
}

fn enable_proposed_repository(rootfs_dir: &PathBuf) -> Result<()> {
    let apt_add_repo_args = vec![
        "-D",
        rootfs_dir.to_str().unwrap(),
        "apt-add-repository",
        "--yes",
        "--no-update",
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

fn install_package(
    rootfs_dir: &PathBuf,
    package_name: &str,
    release: &str,
    proposed: bool,
    ppa: Option<String>,
) -> Result<()> {
    let package_name = if proposed {
        println!("Enabling -proposed repository...");
        enable_proposed_repository(&rootfs_dir)?;
        &format!("{}/{}-proposed", package_name, release)
    } else {
        package_name
    };

    if let Some(ppa_name) = ppa.as_ref() {
        println!("Adding ppa {}", &ppa_name);
        add_ppa(&rootfs_dir, &ppa_name)?;
    }

    run_command(
        "systemd-nspawn",
        &[
            "-D",
            rootfs_dir.to_str().unwrap(),
            "apt-get",
            "update",
            "-y",
        ],
        &format!("Failed to install package {}", package_name),
    )?;

    run_command(
        "systemd-nspawn",
        &[
            "-D",
            rootfs_dir.to_str().unwrap(),
            "apt-get",
            "install",
            "-y",
            package_name,
        ],
        &format!("Failed to install package {}", package_name),
    )?;

    if proposed {
        println!("Disabling -proposed repository...");
        disable_proposed_repository(&rootfs_dir)?;
    }
    if let Some(ppa_name) = ppa.as_ref() {
        println!("Removing PPA {}", &ppa_name);
        remove_ppa(&rootfs_dir, &ppa_name)?;
    }
    Ok(())
}

fn generate_lxd_metadata(package_name: &str, release: &str, proposed: bool) -> Result<()> {
    let lxd_metadata = format!(
        r#"architecture: x86_64
creation_date: {}
properties:
  description: "Ubuntu {} with {}{}"
  os: Ubuntu
  release: "{}"
"#,
        Utc::now().timestamp(),
        release,
        package_name,
        if proposed { " (proposed)" } else { "" },
        release
    );

    fs::write("metadata.yaml", lxd_metadata).context("Failed to write LXD metadata")
}

fn create_lxd_tarball(
    image_path: PathBuf,
    package_name: &str,
    release: &str,
    proposed: bool,
) -> Result<()> {
    // Generate LXD metadata
    generate_lxd_metadata(package_name, release, proposed)?;

    let tarball_name = image_path.clone().with_extension("tar.gz");
    run_command(
        "tar",
        &[
            "--transform",
            &format!("flags=r;s/.*.img/rootfs.img/"),
            "-czf",
            tarball_name.to_str().unwrap(),
            "metadata.yaml",
            image_path.to_str().unwrap(),
        ],
        "Failed to create LXD tarball",
    )?;

    fs::remove_file("metadata.yaml").context("Failed to remove temporary metadata file")?;

    Ok(())
}

/// Customize an Ubuntu cloud image by installing a package from the proposed repository.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable the -proposed repository and install the package from it
    #[arg(long, short, default_value_t = false)]
    proposed: bool,

    /// Create an LXD tarball instead of a QCOW2 image
    #[arg(long, short, default_value_t = false)]
    lxd: bool,

    /// URL or path to the Ubuntu cloud image
    image_uri: String,
    /// Name of the package to install from -proposed
    package_name: String,

    /// Format of the binary image (qcow2, raw, vpc...)
    #[arg(long, default_value_t = String::from("qcow2"))]
    image_format: String,

    /// Enable this PPA before installing package
    #[arg(long, value_name = "ppa:owner/name")]
    ppa: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set the panic hook to print the error message
    std::panic::set_hook(Box::new(|panic_info| {
        eprintln!("Panic occurred: {:?}", panic_info);
    }));

    let cli = Cli::parse();

    // Call the customize_image function
    let image_info = customize_image(
        &cli.image_uri,
        &cli.image_format,
        &cli.package_name,
        cli.proposed,
        cli.ppa,
    )
    .await?;

    if cli.lxd {
        if cli.image_format != "qcow2" {
            return Err(anyhow!(
                "Cannot create LXD tarbal from '{}' image",
                cli.image_format
            ));
        }

        // Generate LXD metadata
        create_lxd_tarball(
            image_info.image_path.clone(),
            &cli.package_name,
            &image_info.release,
            cli.proposed,
        )?;
        fs::remove_file(image_info.image_path).context("Failed to remove temporary image file")?;
    }

    Ok(())
}

struct ImageInfo {
    image_path: PathBuf,
    release: String,
}

async fn customize_image(
    image_uri: &str,
    image_format: &str,
    package_name: &str,
    proposed: bool,
    ppa: Option<String>,
) -> Result<ImageInfo> {
    println!("Starting VM image processing for URL: {}", image_uri);
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
    let image_name = image_uri
        .split('/')
        .last()
        .unwrap()
        .trim_end_matches(".img");

    // Copy image to current directory
    // file format: {image_name}-{package_name}-proposed.img
    let proposed_tag = if proposed { "_proposed" } else { "" };
    let final_image_path = PathBuf::from(format!(
        "{}_{}{}.img",
        image_name, package_name, proposed_tag
    ));

    // Determine if image_url is a URL or a local file path
    if image_uri.starts_with("http://") || image_uri.starts_with("https://") {
        // Download the image from the URL
        println!("Downloading image from URL: {}", image_uri);
        download_image(image_uri, &image_path).await?;
    } else {
        // Treat as local file path, copy to image_path
        println!("Using local image file: {}", image_uri);
        run_command(
            "cp",
            &[image_uri, image_path.to_str().unwrap()],
            &format!("Failed to copy local image file from {}", image_uri),
        )?;
    }

    let release: String;

    // Use a block to ensure `cleanup_guard` is dropped at the end of `main`
    {
        // Mutate the cleanup_guard within the block
        let mut cleanup_guard = cleanup_guard;
        let nbd_device_path = "/dev/nbd0";

        println!("Attaching image to loop device using qemu-nbd");
        connect_image_to_nbd(&image_path, image_format, nbd_device_path)?;

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
        // Determine the release name from the image URL
        release = get_release(&rootfs_dir)?;

        // Install the specified package
        println!("Installing package: {}...", package_name);
        install_package(&rootfs_dir, package_name, &release, proposed, ppa)?;
        println!("Package '{}' installed successfully.", package_name);

        restore_dns(&rootfs_dir)?;
    } // `cleanup_guard` is dropped here, triggering cleanup

    run_command(
        "cp",
        &[
            image_path.to_str().unwrap(),
            final_image_path.to_str().unwrap(),
        ],
        "Failed to copy final image.",
    )?;

    Ok(ImageInfo {
        image_path: final_image_path,
        release,
    })
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
