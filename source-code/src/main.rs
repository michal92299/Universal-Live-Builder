use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use log::{error, info, LevelFilter};
use serde::Deserialize;
use simplelog::{Config, TermLogger, WriteLogger};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use toml;
use walkdir::WalkDir;

// Define the Profile struct based on TOML fields
#[derive(Deserialize, Debug, Clone)]
struct Profile {
    packages: Vec<String>,
    distro_name: String,
    base: String,
    version: String,
    init_system: String,
    packages_to_remove: Vec<String>,
    bootloader: String,
    uefi_support: bool,
    bios_support: bool,
    format: String, // e.g., "iso"
    atomic: bool,   // Whether it's atomic distro or classic
}

#[derive(Parser)]
#[command(name = "Universal Live Builder")]
#[command(version = "1.0")]
#[command(about = "Tool for building custom Linux distributions", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the distro
    Build {
        /// TOML profile file name (optional if only one exists)
        profile: Option<String>,
    },
    /// Clean temporary files
    Clean,
    /// Show tutorials
    Tutorials,
    /// Configure settings like language
    Settings,
    /// Interactive build mode
    ShowBuild,
}

fn main() -> Result<()> {
    // Initialize logging
    let log_dir = PathBuf::from("/tmp/.ulb/logs");
    fs::create_dir_all(&log_dir).context("Failed to create log directory")?;
    let log_path = log_dir.join("ulb.log");
    let log_file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(&log_path)
        .context("Failed to open log file")?;

    TermLogger::init(LevelFilter::Info, Config::default(), simplelog::TerminalMode::Mixed, simplelog::ColorChoice::Auto)
        .context("Failed to initialize term logger")?;
    WriteLogger::init(LevelFilter::Info, Config::default(), log_file).context("Failed to initialize write logger")?;

    info!("Starting Universal Live Builder (ULB)");

    let cli = Cli::parse();

    let current_dir = std::env::current_dir().context("Failed to get current directory")?;
    let profiles_dir = current_dir.join("profiles");
    let files_dir = current_dir.join("files");
    let scripts_dir = current_dir.join("scripts");
    let build_dir = current_dir.join("build/iso");

    fs::create_dir_all(&build_dir).context("Failed to create build directory")?;

    match cli.command {
        Commands::Build { profile } => {
            build_distro(
                &profiles_dir,
                profile.as_deref(),
                &files_dir,
                &scripts_dir,
                &build_dir,
            )?;
        }
        Commands::Clean => clean_tmp()?,
        Commands::Tutorials => show_tutorials(),
        Commands::Settings => configure_settings()?,
        Commands::ShowBuild => {
            interactive_build(&profiles_dir, &files_dir, &scripts_dir, &build_dir)?;
        }
    }

    info!("ULB execution completed");
    Ok(())
}

fn build_distro(
    profiles_dir: &Path,
    profile_name: Option<&str>,
    files_dir: &Path,
    scripts_dir: &Path,
    build_dir: &Path,
) -> Result<()> {
    let profile_path = find_profile(profiles_dir, profile_name)?;
    println!(
        "{}",
        format!("Using profile: {}", profile_path.display()).green()
    );

    let profile_content = fs::read_to_string(&profile_path)
        .context(format!("Failed to read profile: {}", profile_path.display()))?;
    let profile: Profile = toml::from_str(&profile_content).context("Failed to parse TOML")?;

    info!("Parsed profile: {:?}", profile);

    // Setup Podman container for build tools
    setup_podman_container(&profile)?;

    // Prepare rootfs
    let rootfs = PathBuf::from("/tmp/.ulb/rootfs");
    fs::create_dir_all(&rootfs).context("Failed to create rootfs directory")?;

    // Install base system based on 'base'
    install_base_system(&profile, &rootfs)?;

    // Install packages
    install_packages(&profile, &rootfs)?;

    // Remove packages
    remove_packages(&profile, &rootfs)?;

    // Copy files
    copy_files(files_dir, &rootfs)?;

    // Run scripts
    run_scripts(scripts_dir, &rootfs)?;

    // Configure bootloader, init, etc.
    configure_system(&profile, &rootfs)?;

    // Build ISO
    build_iso(&profile, &rootfs, build_dir)?;

    println!("{}", "Build completed!".green());
    Ok(())
}

fn find_profile(profiles_dir: &Path, profile_name: Option<&str>) -> Result<PathBuf> {
    let mut profiles = Vec::new();
    for entry in WalkDir::new(profiles_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.path().extension().and_then(|s| s.to_str()) == Some("toml") {
            profiles.push(entry.path().to_path_buf());
        }
    }

    if profiles.is_empty() {
        return Err(anyhow::anyhow!("No profiles found in {}", profiles_dir.display()));
    }

    if let Some(name) = profile_name {
        let target = profiles_dir.join(if name.ends_with(".toml") { name.to_string() } else { format!("{}.toml", name) });
        if profiles.iter().any(|p| p == &target) {
            Ok(target)
        } else {
            Err(anyhow::anyhow!("Profile '{}' not found", name))
        }
    } else if profiles.len() == 1 {
        Ok(profiles[0].clone())
    } else {
        Err(anyhow::anyhow!("Multiple profiles found, please specify one"))
    }
}

fn setup_podman_container(profile: &Profile) -> Result<()> {
    println!("{}", "Setting up Podman container...".yellow());

    if !Command::new("podman")
        .arg("--version")
        .status()
        .context("Failed to check podman version")?
        .success()
    {
        return Err(anyhow::anyhow!("Podman not found. Please install Podman."));
    }

    let container_dir = PathBuf::from("/tmp/.ulb/build-files");
    fs::create_dir_all(&container_dir).context("Failed to create container directory")?;

    // Pull base image based on profile.base
    let base_image = match profile.base.as_str() {
        "ubuntu" | "debian" => "ubuntu:latest",
        "fedora" => "fedora:latest",
        _ => "ubuntu:latest",
    };
    Command::new("podman")
        .args(&["pull", base_image])
        .status()
        .context("Failed to pull base image")?;

    // Install required tools in container
    let tools = if profile.atomic {
        vec!["ostree", "rpm-ostree", "xorriso"] // For atomic
    } else {
        vec!["debootstrap", "live-build", "xorriso", "lorax"]
    };

    let pkg_manager = if profile.base == "fedora" { "dnf" } else { "apt" };
    let install_cmd = if pkg_manager == "apt" {
        format!("apt update && apt install -y {}", tools.join(" "))
    } else {
        format!("dnf install -y {}", tools.join(" "))
    };

    Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "-v",
            &format!("{}:/build:z", container_dir.display()),
            base_image,
            "bash",
            "-c",
            &install_cmd,
        ])
        .status()
        .context("Failed to install tools in container")?;

    info!("Podman container setup complete");
    Ok(())
}

fn install_base_system(profile: &Profile, rootfs: &Path) -> Result<()> {
    println!("{}", "Installing base system...".yellow());

    let base_cmd = match profile.base.as_str() {
        "debian" | "ubuntu" => "debootstrap",
        "fedora" if profile.atomic => "rpm-ostree",
        _ => return Err(anyhow::anyhow!("Unsupported base: {}", profile.base)),
    };

    // Example for debootstrap
    if base_cmd == "debootstrap" {
        Command::new("podman")
            .args(&[
                "run",
                "--rm",
                "-v",
                &format!("{}:/rootfs:z", rootfs.display()),
                "ubuntu:latest",
                "debootstrap",
                "--arch=amd64",
                "stable",
                "/rootfs",
                "http://deb.debian.org/debian/",
            ])
            .status()
            .context("Failed to run debootstrap")?;
    } else if base_cmd == "rpm-ostree" {
        // Placeholder for atomic
        println!("{}", "Atomic base installation (placeholder)".cyan());
    }

    Ok(())
}

fn install_packages(profile: &Profile, rootfs: &Path) -> Result<()> {
    println!("{}", "Installing packages...".yellow());

    if !profile.packages.is_empty() {
        let pkg_manager = if profile.base == "fedora" { "dnf" } else { "apt" };
        let install_cmd = format!("{} install -y {}", pkg_manager, profile.packages.join(" "));

        Command::new("podman")
            .args(&[
                "run",
                "--rm",
                "-v",
                &format!("{}:/rootfs:z", rootfs.display()),
                "ubuntu:latest",  // Adjust if fedora
                "chroot",
                "/rootfs",
                "bash",
                "-c",
                &install_cmd,
            ])
            .status()
            .context("Failed to install packages")?;
    }

    Ok(())
}

fn remove_packages(profile: &Profile, rootfs: &Path) -> Result<()> {
    if !profile.packages_to_remove.is_empty() {
        println!("{}", "Removing packages...".yellow());
        let pkg_manager = if profile.base == "fedora" { "dnf" } else { "apt" };
        let remove_cmd = format!("{} remove -y {}", pkg_manager, profile.packages_to_remove.join(" "));

        Command::new("podman")
            .args(&[
                "run",
                "--rm",
                "-v",
                &format!("{}:/rootfs:z", rootfs.display()),
                "ubuntu:latest",
                "chroot",
                "/rootfs",
                "bash",
                "-c",
                &remove_cmd,
            ])
            .status()
            .context("Failed to remove packages")?;
    }
    Ok(())
}

fn copy_files(src_dir: &Path, dest_dir: &Path) -> Result<()> {
    if src_dir.exists() {
        println!("{}", "Copying files...".yellow());
        for entry in WalkDir::new(src_dir) {
            let entry = entry.context("Failed to walk dir")?;
            let relative = entry.path().strip_prefix(src_dir).unwrap();
            let dest = dest_dir.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&dest).context("Failed to create dir")?;
            } else {
                fs::copy(entry.path(), &dest).context("Failed to copy file")?;
            }
        }
    }
    Ok(())
}

fn run_scripts(scripts_dir: &Path, rootfs: &Path) -> Result<()> {
    if scripts_dir.exists() {
        println!("{}", "Running scripts...".yellow());
        let mut scripts: Vec<_> = fs::read_dir(scripts_dir)
            .context("Failed to read scripts dir")?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "sh"))
            .collect();
        
        // Sort scripts alphabetically to ensure consistent order
        scripts.sort_by_key(|e| e.file_name());

        for entry in scripts {
            info!("Running script: {}", entry.path().display());
            Command::new("podman")
                .args(&[
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/rootfs:z", rootfs.display()),
                    "-v",
                    &format!("{}:/script.sh:z,ro", entry.path().display()),
                    "ubuntu:latest",
                    "chroot",
                    "/rootfs",
                    "bash",
                    "/script.sh",
                ])
                .status()
                .context(format!("Failed to run script: {}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn configure_system(profile: &Profile, rootfs: &Path) -> Result<()> {
    println!("{}", "Configuring system...".yellow());

    // Configure init system
    match profile.init_system.as_str() {
        "systemd" => {
            // Enable systemd
            Command::new("podman")
                .args(&[
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/rootfs:z", rootfs.display()),
                    "ubuntu:latest",
                    "chroot",
                    "/rootfs",
                    "systemctl",
                    "enable",
                    "systemd-sysv-install",
                ])
                .status()
                .context("Failed to configure systemd")?;
        }
        "openrc" => {
            // Placeholder
            println!("{}", "OpenRC configuration (placeholder)".cyan());
        }
        _ => error!("Unsupported init system: {}", profile.init_system),
    }

    // Configure bootloader
    let bootloader_cmd = match profile.bootloader.as_str() {
        "grub" => "grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=GRUB",
        "systemd-boot" => "bootctl --path=/boot install",
        _ => return Err(anyhow::anyhow!("Unsupported bootloader: {}", profile.bootloader)),
    };

    Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "-v",
            &format!("{}:/rootfs:z", rootfs.display()),
            "ubuntu:latest",
            "chroot",
            "/rootfs",
            "bash",
            "-c",
            bootloader_cmd,
        ])
        .status()
        .context("Failed to install bootloader")?;

    // Handle UEFI/BIOS support
    if !profile.uefi_support && !profile.bios_support {
        return Err(anyhow::anyhow!("Must support at least UEFI or BIOS"));
    }
    // Additional config if needed

    Ok(())
}

fn build_iso(profile: &Profile, rootfs: &Path, build_dir: &Path) -> Result<()> {
    println!("{}", "Building ISO...".yellow());

    let iso_path = build_dir.join(format!("{}-{}.iso", profile.distro_name, profile.version));

    let build_cmd = if profile.atomic {
        // Placeholder for atomic build, e.g., rpm-ostree compose
        "rpm-ostree compose tree --repo=/rootfs/ostree-repo /rootfs/tree.yaml && xorriso -as mkisofs -o /output.iso /rootfs"
    } else {
        // For classic, use live-build or mksquashfs + xorriso
        "mksquashfs /rootfs /filesystem.squashfs && xorriso -as mkisofs -o /output.iso -b isolinux/isolinux.bin -c isolinux/boot.cat -no-emul-boot -boot-load-size 4 -boot-info-table -eltorito-alt-boot -e boot/efi.img -no-emul-boot /rootfs"
    };

    Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "-v",
            &format!("{}:/rootfs:z", rootfs.display()),
            "-v",
            &format!("{}:/output.iso:z", iso_path.display()),
            "ubuntu:latest",
            "bash",
            "-c",
            build_cmd,
        ])
        .status()
        .context("Failed to build ISO")?;

    info!("ISO built at {}", iso_path.display());
    Ok(())
}

fn clean_tmp() -> Result<()> {
    println!("{}", "Cleaning temporary files...".yellow());
    let ulb_tmp = Path::new("/tmp/.ulb");
    if ulb_tmp.exists() {
        fs::remove_dir_all(ulb_tmp).context("Failed to remove /tmp/.ulb")?;
    }
    println!("{}", "Cleaned!".green());
    Ok(())
}

fn show_tutorials() {
    println!("{}", "Tutorials:".blue());
    println!("1. Create a profile.toml in /profiles with fields:");
    println!("   - packages: list of packages to install");
    println!("   - distro_name: name of your distro");
    println!("   - base: base distro (e.g., ubuntu, fedora)");
    println!("   - version: version string");
    println!("   - init_system: systemd or openrc");
    println!("   - packages_to_remove: list to remove");
    println!("   - bootloader: grub or systemd-boot");
    println!("   - uefi_support: true/false");
    println!("   - bios_support: true/false");
    println!("   - format: iso");
    println!("   - atomic: true for atomic, false for classic");
    println!("2. Add files to /files to overlay on /");
    println!("3. Add executable .sh scripts to /scripts (run in order)");
    println!("4. Run 'ulb build' or 'ulb build profile_name'");
    println!("For atomic distros, use fedora base and set atomic=true.");
}

fn configure_settings() -> Result<()> {
    println!("{}", "Settings:".blue());
    println!("Current language: English");
    println!("To change language (future feature): select from menu.");
    // Placeholder for settings, e.g., language
    Ok(())
}

fn interactive_build(
    profiles_dir: &Path,
    files_dir: &Path,
    scripts_dir: &Path,
    build_dir: &Path,
) -> Result<()> {
    println!("{}", "Interactive Build Mode".blue());
    println!("Answer questions to create a profile. Type 'back' to go back.");

    let mut profile = Profile {
        distro_name: prompt("Distro name: ")?,
        base: prompt("Base (ubuntu, debian, fedora): ")?,
        version: prompt("Version: ")?,
        init_system: prompt("Init system (systemd, openrc): ")?,
        bootloader: prompt("Bootloader (grub, systemd-boot): ")?,
        uefi_support: prompt_bool("UEFI support? (y/n): ")?,
        bios_support: prompt_bool("BIOS support? (y/n): ")?,
        format: "iso".to_string(),
        atomic: prompt_bool("Atomic distro? (y/n): ")?,
        packages: prompt_list("Packages to install (comma-separated): ")?,
        packages_to_remove: prompt_list("Packages to remove (comma-separated): ")?,
    };

    // Validate
    if profile.base == "fedora" && !profile.atomic {
        println!("{}", "Warning: Fedora recommended for atomic.".yellow());
    }

    // Save to temp TOML
    let temp_profile_path = profiles_dir.join("interactive.toml");
    let toml_str = toml::to_string(&profile).context("Failed to serialize profile")?;
    fs::write(&temp_profile_path, toml_str).context("Failed to write temp profile")?;

    // Build
    build_distro(profiles_dir, Some("interactive"), files_dir, scripts_dir, build_dir)?;

    // Cleanup
    fs::remove_file(&temp_profile_path).context("Failed to remove temp profile")?;

    Ok(())
}

fn prompt(question: &str) -> Result<String> {
    loop {
        print!("{}", question.yellow());
        io::stdout().flush().context("Failed to flush stdout")?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("Failed to read line")?;
        let trimmed = input.trim();
        if trimmed == "back" {
            // Handle back, but for simplicity, just reprompt
            continue;
        }
        return Ok(trimmed.to_string());
    }
}

fn prompt_bool(question: &str) -> Result<bool> {
    let answer = prompt(question)?;
    Ok(answer.to_lowercase() == "y")
}

fn prompt_list(question: &str) -> Result<Vec<String>> {
    let input = prompt(question)?;
    Ok(input.split(',').map(|s| s.trim().to_string()).collect())
}
