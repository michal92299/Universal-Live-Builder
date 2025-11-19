use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use log::{error, info, LevelFilter};
use serde::{Deserialize, Serialize};
use simplelog::{Config, TermLogger, WriteLogger};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use toml;
use walkdir::WalkDir;

// Define the Profile struct based on TOML fields
#[derive(Deserialize, Serialize, Debug, Clone)]
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
    /// Initialize a new project with example structure
    Init,
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

    match cli.command {
        Commands::Build { profile } => {
            fs::create_dir_all(&build_dir).context("Failed to create build directory")?;
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
            fs::create_dir_all(&build_dir).context("Failed to create build directory")?;
            interactive_build(&profiles_dir, &files_dir, &scripts_dir, &build_dir)?;
        }
        Commands::Init => init_project(&current_dir)?,
    }

    info!("ULB execution completed");
    Ok(())
}

fn init_project(current_dir: &Path) -> Result<()> {
    println!("{}", "Initializing project...".yellow());

    fs::create_dir_all(current_dir.join("profiles")).context("Failed to create profiles dir")?;
    fs::create_dir_all(current_dir.join("files")).context("Failed to create files dir")?;
    fs::create_dir_all(current_dir.join("scripts")).context("Failed to create scripts dir")?;
    fs::create_dir_all(current_dir.join("build/iso")).context("Failed to create build/iso dir")?;

    let example_toml = r#"
packages = ["vim", "git"]
distro_name = "MyDistro"
base = "ubuntu"
version = "1.0"
init_system = "systemd"
packages_to_remove = []
bootloader = "grub"
uefi_support = true
bios_support = true
format = "iso"
atomic = false
"#;

    let profile_path = current_dir.join("profiles/example.toml");
    fs::write(&profile_path, example_toml).context("Failed to write example.toml")?;

    println!("{}", "Project initialized with example profile!".green());
    println!("Folders created: profiles, files, scripts, build/iso");
    println!("Example profile: profiles/example.toml");
    println!("You can now run 'ulb build example' to build.");

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
        return Err(anyhow::anyhow!("No profiles found in {}. Run 'ulb init' to create an example.", profiles_dir.display()));
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
        _ => return Err(anyhow::anyhow!("Unsupported base: {}. Supported: ubuntu, debian, fedora", profile.base)),
    };
    let output = Command::new("podman")
        .args(&["pull", base_image])
        .output()
        .context("Failed to pull base image")?;
    if !output.status.success() {
        error!("Podman pull failed: {}", String::from_utf8_lossy(&output.stderr));
        return Err(anyhow::anyhow!("Failed to pull image"));
    }

    // Install required tools in container
    let tools = if profile.atomic {
        vec!["ostree", "rpm-ostree", "xorriso", "mksquashfs"] // For atomic
    } else {
        vec!["debootstrap", "live-build", "xorriso", "lorax", "mksquashfs"]
    };

    let pkg_manager = if profile.base == "fedora" { "dnf" } else { "apt" };
    let install_cmd = if pkg_manager == "apt" {
        format!("apt update && apt install -y {}", tools.join(" "))
    } else {
        format!("dnf install -y {}", tools.join(" "))
    };

    let output = Command::new("podman")
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
        .output()
        .context("Failed to install tools in container")?;
    if !output.status.success() {
        error!("Tool installation failed: {}", String::from_utf8_lossy(&output.stderr));
        return Err(anyhow::anyhow!("Failed to install tools"));
    }

    info!("Podman container setup complete");
    Ok(())
}

fn install_base_system(profile: &Profile, rootfs: &Path) -> Result<()> {
    println!("{}", "Installing base system...".yellow());

    let base_image = match profile.base.as_str() {
        "ubuntu" | "debian" => "ubuntu:latest",
        "fedora" => "fedora:latest",
        _ => unreachable!(),
    };

    let base_cmd = match profile.base.as_str() {
        "debian" | "ubuntu" => "debootstrap",
        "fedora" if profile.atomic => "rpm-ostree",
        "fedora" => "dnf",
        _ => return Err(anyhow::anyhow!("Unsupported base: {}", profile.base)),
    };

    let install_cmd = match base_cmd {
        "debootstrap" => {
            format!("debootstrap --arch=amd64 stable /rootfs http://deb.debian.org/debian/")
        }
        "rpm-ostree" => {
            // Placeholder for atomic Fedora
            "rpm-ostree install --repo=/rootfs/ostree-repo base-packages".to_string()
        }
        "dnf" => {
            format!("dnf install -y --installroot=/rootfs --releasever=latest @core")
        }
        _ => unreachable!(),
    };

    let output = Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "--privileged",  // May need for some installs
            "-v",
            &format!("{}:/rootfs:z", rootfs.display()),
            base_image,
            "bash",
            "-c",
            &install_cmd,
        ])
        .output()
        .context("Failed to run base install")?;
    if !output.status.success() {
        error!("Base install failed: {}", String::from_utf8_lossy(&output.stderr));
        return Err(anyhow::anyhow!("Base system installation failed"));
    }

    Ok(())
}

fn install_packages(profile: &Profile, rootfs: &Path) -> Result<()> {
    if !profile.packages.is_empty() {
        println!("{}", "Installing packages...".yellow());

        let base_image = match profile.base.as_str() {
            "ubuntu" | "debian" => "ubuntu:latest",
            "fedora" => "fedora:latest",
            _ => unreachable!(),
        };

        let pkg_manager = if profile.base == "fedora" { "dnf" } else { "apt" };
        let install_cmd = format!("{} install -y {}", pkg_manager, profile.packages.join(" "));

        let output = Command::new("podman")
            .args(&[
                "run",
                "--rm",
                "-v",
                &format!("{}:/rootfs:z", rootfs.display()),
                base_image,
                "chroot",
                "/rootfs",
                "bash",
                "-c",
                &install_cmd,
            ])
            .output()
            .context("Failed to install packages")?;
        if !output.status.success() {
            error!("Package install failed: {}", String::from_utf8_lossy(&output.stderr));
            return Err(anyhow::anyhow!("Package installation failed"));
        }
    }

    Ok(())
}

fn remove_packages(profile: &Profile, rootfs: &Path) -> Result<()> {
    if !profile.packages_to_remove.is_empty() {
        println!("{}", "Removing packages...".yellow());

        let base_image = match profile.base.as_str() {
            "ubuntu" | "debian" => "ubuntu:latest",
            "fedora" => "fedora:latest",
            _ => unreachable!(),
        };

        let pkg_manager = if profile.base == "fedora" { "dnf" } else { "apt" };
        let remove_cmd = format!("{} remove -y {}", pkg_manager, profile.packages_to_remove.join(" "));

        let output = Command::new("podman")
            .args(&[
                "run",
                "--rm",
                "-v",
                &format!("{}:/rootfs:z", rootfs.display()),
                base_image,
                "chroot",
                "/rootfs",
                "bash",
                "-c",
                &remove_cmd,
            ])
            .output()
            .context("Failed to remove packages")?;
        if !output.status.success() {
            error!("Package remove failed: {}", String::from_utf8_lossy(&output.stderr));
            return Err(anyhow::anyhow!("Package removal failed"));
        }
    }
    Ok(())
}

fn copy_files(src_dir: &Path, dest_dir: &Path) -> Result<()> {
    if src_dir.exists() {
        println!("{}", "Copying files...".yellow());
        for entry in WalkDir::new(src_dir) {
            let entry = entry.context("Failed to walk dir")?;
            let relative = entry.path().strip_prefix(src_dir).context("Failed to strip prefix")?;
            let dest = dest_dir.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&dest).context("Failed to create dir")?;
            } else {
                fs::copy(entry.path(), &dest).context(format!("Failed to copy file {}", entry.path().display()))?;
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

        let base_image = "ubuntu:latest"; // Adjust if needed

        for entry in scripts {
            info!("Running script: {}", entry.path().display());
            let output = Command::new("podman")
                .args(&[
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/rootfs:z", rootfs.display()),
                    "-v",
                    &format!("{}:/script.sh:z,ro", entry.path().display()),
                    base_image,
                    "chroot",
                    "/rootfs",
                    "bash",
                    "/script.sh",
                ])
                .output()
                .context(format!("Failed to run script: {}", entry.path().display()))?;
            if !output.status.success() {
                error!("Script failed: {}", String::from_utf8_lossy(&output.stderr));
                return Err(anyhow::anyhow!("Script execution failed"));
            }
        }
    }
    Ok(())
}

fn configure_system(profile: &Profile, rootfs: &Path) -> Result<()> {
    println!("{}", "Configuring system...".yellow());

    let base_image = match profile.base.as_str() {
        "ubuntu" | "debian" => "ubuntu:latest",
        "fedora" => "fedora:latest",
        _ => unreachable!(),
    };

    // Configure init system
    let init_cmd = match profile.init_system.as_str() {
        "systemd" => "systemctl enable systemd-sysv-install",
        "openrc" => "rc-update add ...", // Placeholder
        _ => return Err(anyhow::anyhow!("Unsupported init system: {}", profile.init_system)),
    };

    let output = Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "-v",
            &format!("{}:/rootfs:z", rootfs.display()),
            base_image,
            "chroot",
            "/rootfs",
            "bash",
            "-c",
            init_cmd,
        ])
        .output()
        .context("Failed to configure init")?;
    if !output.status.success() {
        error!("Init config failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    // Configure bootloader
    let bootloader_cmd = match profile.bootloader.as_str() {
        "grub" => "grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=GRUB",
        "systemd-boot" => "bootctl --path=/boot install",
        _ => return Err(anyhow::anyhow!("Unsupported bootloader: {}", profile.bootloader)),
    };

    let output = Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "--privileged",
            "-v",
            &format!("{}:/rootfs:z", rootfs.display()),
            base_image,
            "chroot",
            "/rootfs",
            "bash",
            "-c",
            bootloader_cmd,
        ])
        .output()
        .context("Failed to install bootloader")?;
    if !output.status.success() {
        error!("Bootloader install failed: {}", String::from_utf8_lossy(&output.stderr));
        return Err(anyhow::anyhow!("Bootloader configuration failed"));
    }

    // Handle UEFI/BIOS support
    if !profile.uefi_support && !profile.bios_support {
        return Err(anyhow::anyhow!("Must support at least UEFI or BIOS"));
    }
    // Additional config if needed, e.g., generate initramfs

    let mkinit_cmd = if profile.base == "fedora" {
        "dracut -f /boot/initramfs.img"
    } else {
        "update-initramfs -u"
    };

    let output = Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "-v",
            &format!("{}:/rootfs:z", rootfs.display()),
            base_image,
            "chroot",
            "/rootfs",
            "bash",
            "-c",
            mkinit_cmd,
        ])
        .output()
        .context("Failed to generate initramfs")?;
    if !output.status.success() {
        error!("Initramfs failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

fn build_iso(profile: &Profile, rootfs: &Path, build_dir: &Path) -> Result<()> {
    println!("{}", "Building ISO...".yellow());

    let iso_path = build_dir.join(format!("{}-{}.iso", profile.distro_name, profile.version));
    let tmp_output = PathBuf::from("/tmp/.ulb/output.iso");

    let base_image = match profile.base.as_str() {
        "ubuntu" | "debian" => "ubuntu:latest",
        "fedora" => "fedora:latest",
        _ => unreachable!(),
    };

    let build_cmd = if profile.atomic {
        // Placeholder for atomic build
        "rpm-ostree compose tree --repo=/rootfs/ostree-repo /rootfs/tree.yaml && mksquashfs /rootfs /filesystem.squashfs -comp xz && xorriso -as mkisofs -o /output.iso -V 'MyDistro' -e /filesystem.squashfs -no-emul-boot /rootfs"
    } else {
        // For classic, use mksquashfs + xorriso
        "mksquashfs /rootfs /filesystem.squashfs -comp xz && xorriso -as mkisofs -o /output.iso -b isolinux/isolinux.bin -c isolinux/boot.cat -no-emul-boot -boot-load-size 4 -boot-info-table -eltorito-alt-boot -e boot/efi.img -no-emul-boot -V 'MyDistro' /rootfs"
    };

    let output = Command::new("podman")
        .args(&[
            "run",
            "--rm",
            "--privileged",
            "-v",
            &format!("{}:/rootfs:z", rootfs.display()),
            "-v",
            &format!("{}:/output.iso:z", tmp_output.display()),
            base_image,
            "bash",
            "-c",
            build_cmd,
        ])
        .output()
        .context("Failed to build ISO")?;
    if !output.status.success() {
        error!("ISO build failed: {}", String::from_utf8_lossy(&output.stderr));
        return Err(anyhow::anyhow!("ISO build failed"));
    }

    fs::rename(&tmp_output, &iso_path).context("Failed to move ISO")?;

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
    println!("1. Run 'ulb init' to create project structure.");
    println!("2. Edit profiles/*.toml with your settings.");
    println!("   Fields:");
    println!("   - packages: list of packages to install");
    println!("   - distro_name: name of your distro");
    println!("   - base: base distro (ubuntu, debian, fedora)");
    println!("   - version: version string");
    println!("   - init_system: systemd or openrc");
    println!("   - packages_to_remove: list to remove");
    println!("   - bootloader: grub or systemd-boot");
    println!("   - uefi_support: true/false");
    println!("   - bios_support: true/false");
    println!("   - format: iso (only supported)");
    println!("   - atomic: true for atomic (fedora only), false for classic");
    println!("3. Add files to /files to overlay on rootfs /");
    println!("4. Add .sh scripts to /scripts (executed in alphabetical order post-install)");
    println!("5. Run 'ulb build' or 'ulb build profile_name'");
    println!("6. Output ISO in build/iso");
    println!("7. Use 'ulb clean' to clean /tmp/.ulb");
    println!("8. 'ulb show-build' for interactive mode");
}

fn configure_settings() -> Result<()> {
    println!("{}", "Settings:".blue());
    println!("Current language: English");
    println!("Future features: language selection, custom themes.");
    // Placeholder, could add config file in future
    Ok(())
}

fn interactive_build(
    profiles_dir: &Path,
    files_dir: &Path,
    scripts_dir: &Path,
    build_dir: &Path,
) -> Result<()> {
    println!("{}", "Interactive Build Mode".blue());
    println!("Answer questions to create a profile. Type 'back' to retry question.");

    let mut profile = Profile {
        distro_name: prompt("Distro name (e.g., MyDistro): ")?,
        base: prompt("Base (ubuntu, debian, fedora): ")?,
        version: prompt("Version (e.g., 1.0): ")?,
        init_system: prompt("Init system (systemd, openrc): ")?,
        bootloader: prompt("Bootloader (grub, systemd-boot): ")?,
        uefi_support: prompt_bool("UEFI support? (y/n): ")?,
        bios_support: prompt_bool("BIOS support? (y/n): ")?,
        format: "iso".to_string(),
        atomic: prompt_bool("Atomic distro? (y/n, recommended for fedora): ")?,
        packages: prompt_list("Packages to install (comma-separated, e.g., vim,git): ")?,
        packages_to_remove: prompt_list("Packages to remove (comma-separated): ")?,
    };

    // Basic validation
    if profile.base != "ubuntu" && profile.base != "debian" && profile.base != "fedora" {
        return Err(anyhow::anyhow!("Invalid base: {}", profile.base));
    }
    if profile.atomic && profile.base != "fedora" {
        println!("{}", "Warning: Atomic supported only for fedora.".yellow());
        profile.atomic = false;
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
        let trimmed = input.trim().to_string();
        if trimmed == "back" {
            continue;
        }
        if trimmed.is_empty() {
            println!("{}", "Input cannot be empty.".red());
            continue;
        }
        return Ok(trimmed);
    }
}

fn prompt_bool(question: &str) -> Result<bool> {
    loop {
        let answer = prompt(question)?;
        match answer.to_lowercase().as_str() {
            "y" => return Ok(true),
            "n" => return Ok(false),
            _ => println!("{}", "Please answer y or n.".red()),
        }
    }
}

fn prompt_list(question: &str) -> Result<Vec<String>> {
    let input = prompt(question)?;
    if input.is_empty() {
        return Ok(vec![]);
    }
    Ok(input.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
}
