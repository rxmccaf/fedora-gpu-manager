// fedora-gpu-manager
// GTK4 / libadwaita — GPU driver detection and management for Fedora 44+

use gtk4::prelude::*;
use libadwaita::prelude::*;
use libadwaita as adw;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::io::{BufRead, BufReader};
use std::thread;

const APP_ID:      &str = "org.fedoraproject.GpuManager";
const APP_NAME:    &str = "GPU Driver Manager";
const APP_VERSION: &str = "1.0.0";

// ---------------------------------------------------------------------------
// Detection helpers
// ---------------------------------------------------------------------------

fn run_cmd(cmd: &str) -> String {
    Command::new("bash")
        .args(["-c", cmd])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub slot:           String,
    pub name:           String,
    pub pci_id:         String,
    pub vendor:         Vendor,
    pub driver_in_use:  String,
    pub driver_modules: String,
    pub installed_pkgs: HashMap<String, String>,
    pub available_pkgs: HashMap<String, String>,
    pub extra:          Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Vendor { Nvidia, Amd, Intel, Unknown }

impl Vendor {
    fn from_pci_id(id: &str) -> Self {
        match id.split(':').next().unwrap_or("").to_lowercase().as_str() {
            "10de" => Vendor::Nvidia,
            "1002" => Vendor::Amd,
            "8086" => Vendor::Intel,
            _      => Vendor::Unknown,
        }
    }
    fn label(&self) -> &str {
        match self {
            Vendor::Nvidia  => "NVIDIA",
            Vendor::Amd     => "AMD",
            Vendor::Intel   => "Intel",
            Vendor::Unknown => "Unknown",
        }
    }
    fn subtitle(&self) -> &str {
        match self {
            Vendor::Nvidia  => "Proprietary driver via RPM Fusion nonfree (akmod-nvidia)",
            Vendor::Amd     => "Open-source amdgpu driver (in-kernel) · Mesa via main Fedora repos",
            Vendor::Intel   => "Open-source i915/xe driver (in-kernel) · No additional repos required",
            Vendor::Unknown => "Unknown vendor",
        }
    }
    // Known AMD integrated GPU PCI device IDs
    fn is_amd_igpu(pci_id: &str) -> bool {
        matches!(pci_id.to_lowercase().as_str(),
            "1002:164e" | "1002:164c" |  // Raphael (Zen 4 desktop, 7000 series)
            "1002:1681" | "1002:1680" |  // Rembrandt (Zen 3+, 6000 series)
            "1002:15bf" | "1002:15c8" |  // Phoenix (Zen 4 mobile, 7040 series)
            "1002:15c9" | "1002:15ca" |  // Phoenix 2
            "1002:1900" | "1002:1901"    // Strix / Hawk Point (Zen 5)
        )
    }
}

pub fn detect_gpus() -> Vec<GpuInfo> {
    static BLOCK_SPLIT: Lazy<Regex> = Lazy::new(||
        Regex::new(r"(?m)^([0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-9a-fA-F])").unwrap());
    static GPU_CLASS:   Lazy<Regex> = Lazy::new(||
        Regex::new(r"(?i)\b(VGA|3D|Display)\b").unwrap());
    static SLOT:        Lazy<Regex> = Lazy::new(||
        Regex::new(r"^([0-9a-fA-F:.]+)\s").unwrap());
    static PCIID:       Lazy<Regex> = Lazy::new(||
        Regex::new(r"\[([0-9a-fA-F]{4}:[0-9a-fA-F]{4})\]").unwrap());
    static CTRL:        Lazy<Regex> = Lazy::new(||
        Regex::new(r"(?i)^[0-9a-fA-F:.]+\s+\S+\s+controller:\s*").unwrap());
    static TRAIL_ID:    Lazy<Regex> = Lazy::new(||
        Regex::new(r"\s*\[[0-9a-fA-F:]+\]\s*$").unwrap());
    static DRV_USE:     Lazy<Regex> = Lazy::new(||
        Regex::new(r"^\s+Kernel driver in use:\s*(.+)").unwrap());
    static DRV_MODS:    Lazy<Regex> = Lazy::new(||
        Regex::new(r"^\s+Kernel modules:\s*(.+)").unwrap());

    let raw = run_cmd("lspci -nn -k");
    let mut gpus = Vec::new();

    let matches: Vec<_> = BLOCK_SPLIT.find_iter(&raw).collect();
    let splits: Vec<&str> = BLOCK_SPLIT.split(&raw).skip(1).collect();
    let owned_blocks: Vec<String> = matches.iter().zip(splits.iter())
        .map(|(m, rest)| format!("{}{}", m.as_str(), rest))
        .collect();

    for block in &owned_blocks {
        let first = block.lines().next().unwrap_or("");
        if !GPU_CLASS.is_match(first) { continue; }

        let slot   = SLOT.captures(first)
                         .map(|c| c[1].to_string())
                         .unwrap_or_else(|| "?".into());
        let pci_id = PCIID.captures(first)
                          .map(|c| c[1].to_string())
                          .unwrap_or_else(|| "?".into());
        let name   = TRAIL_ID.replace(&CTRL.replace(first, ""), "")
                              .trim().to_string();
        let vendor = Vendor::from_pci_id(&pci_id);

        let mut driver_in_use  = String::new();
        let mut driver_modules = String::new();
        for line in block.lines() {
            if let Some(c) = DRV_USE.captures(line)  { driver_in_use  = c[1].trim().into(); }
            if let Some(c) = DRV_MODS.captures(line) { driver_modules = c[1].trim().into(); }
        }

        gpus.push(GpuInfo {
            slot, name, pci_id, vendor,
            driver_in_use:  if driver_in_use.is_empty()  { "none".into() } else { driver_in_use },
            driver_modules: if driver_modules.is_empty() { "none".into() } else { driver_modules },
            installed_pkgs: HashMap::new(),
            available_pkgs: HashMap::new(),
            extra:          Vec::new(),
        });
    }
    gpus
}

pub fn rpmfusion_enabled() -> (bool, bool) {
    // Parse repo files directly — faster and more reliable than `dnf repolist enabled`
    let free = !run_cmd(
        "grep -rl 'enabled=1' /etc/yum.repos.d/rpmfusion-free*.repo 2>/dev/null | grep -v nonfree"
    ).is_empty();
    let nonfree = !run_cmd(
        "grep -rl 'enabled=1' /etc/yum.repos.d/rpmfusion-nonfree*.repo 2>/dev/null"
    ).is_empty();
    (free, nonfree)
}

pub fn rpmfusion_installed() -> (bool, bool) {
    // Check if the release packages themselves are installed (different from enabled)
    let free    = !run_cmd("rpm -q rpmfusion-free-release 2>/dev/null | grep -v 'not installed'").is_empty();
    let nonfree = !run_cmd("rpm -q rpmfusion-nonfree-release 2>/dev/null | grep -v 'not installed'").is_empty();
    (free, nonfree)
}

fn query_installed(patterns: &[&str]) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for pat in patterns {
        let out = run_cmd(&format!(
            "rpm -q {pat} --queryformat '%{{NAME}} %{{VERSION}}-%{{RELEASE}}\\n' 2>/dev/null"
        ));
        for line in out.lines() {
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            if parts.len() == 2 && !line.contains("not installed") {
                result.insert(parts[0].to_string(), parts[1].to_string());
            }
        }
    }
    result
}

fn query_available(patterns: &[&str]) -> HashMap<String, String> {
    static LINE_RE: Lazy<Regex> = Lazy::new(||
        Regex::new(r"^(\S+)\.\S+\s+(\S+)\s+").unwrap());
    let mut result = HashMap::new();
    for pat in patterns {
        let out = run_cmd(&format!("dnf list --available --cacheonly {pat} 2>/dev/null"));
        for line in out.lines() {
            if let Some(c) = LINE_RE.captures(line) {
                let name = c[1].to_string();
                let ver  = c[2].to_string();
                if patterns.iter().any(|p| name.contains(&p.replace('*', ""))) {
                    result.insert(name, ver);
                }
            }
        }
    }
    result
}

pub fn enrich_gpu(gpu: &mut GpuInfo) {
    match gpu.vendor {
        Vendor::Nvidia => {
            let smi = run_cmd(
                "nvidia-smi --query-gpu=driver_version,vbios_version \
                 --format=csv,noheader 2>/dev/null | head -1");
            if !smi.is_empty() {
                let parts: Vec<&str> = smi.split(',').map(str::trim).collect();
                if let Some(v) = parts.first() {
                    gpu.extra.push(("nvidia-smi driver".into(), v.to_string()));
                }
                if parts.len() > 1 {
                    gpu.extra.push(("VBIOS version".into(), parts[1].to_string()));
                }
            }
            gpu.installed_pkgs = query_installed(&[
                "akmod-nvidia",
                "xorg-x11-drv-nvidia",
                "xorg-x11-drv-nvidia-power",
                "nvidia-settings",
                "nvidia-modprobe",
                "nvidia-xconfig",
                "libva-nvidia-driver",
            ]);
            gpu.available_pkgs = query_available(&["akmod-nvidia", "xorg-x11-drv-nvidia"]);
        }
        Vendor::Amd => {
            let vk = run_cmd(
                "vulkaninfo --summary 2>/dev/null | grep -iE 'deviceName|driverVersion' | head -2");
            let mut seen = std::collections::HashSet::new();
            for line in vk.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    let key = k.trim().to_string();
                    if seen.insert(key.clone()) {
                        gpu.extra.push((key, v.trim().to_string()));
                    }
                }
            }
            gpu.installed_pkgs = query_installed(&[
                "mesa-dri-drivers",
                "mesa-vulkan-drivers",
                "xorg-x11-drv-amdgpu",
            ]);
            gpu.available_pkgs = query_available(&[
                "mesa-dri-drivers",
                "mesa-vulkan-drivers",
                "xorg-x11-drv-amdgpu",
            ]);
        }
        Vendor::Intel => {
            gpu.installed_pkgs = query_installed(&[
                "intel-media-driver",
                "libva-intel-driver",
                "xorg-x11-drv-intel",
            ]);
            gpu.available_pkgs = query_available(&[
                "intel-media-driver",
                "libva-intel-driver",
            ]);
        }
        Vendor::Unknown => {}
    }
}

// ---------------------------------------------------------------------------
// RPM Fusion install command (full URL install for fresh systems)
// ---------------------------------------------------------------------------

pub fn install_rpmfusion_cmd() -> String {
    let ver = run_cmd("rpm -E %fedora");
    let ver = if ver.is_empty() { "$(rpm -E %fedora)".into() } else { ver };
    format!(
        "dnf install -y \
         https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-{ver}.noarch.rpm \
         https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-{ver}.noarch.rpm && \
         dnf config-manager setopt fedora-cisco-openh264.enabled=1"
    )
}

// ---------------------------------------------------------------------------
// Generic pkexec runner — runs any command via pkexec, streams to log channel
// ---------------------------------------------------------------------------

fn spawn_pkexec(cmd: String, tx: async_channel::Sender<String>) {
    let _ = tx.send_blocking(format!("▶ {cmd}\n\n"));
    thread::spawn(move || {
        let child = Command::new("pkexec")
            .args(["bash", "-c", &cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        match child {
            Err(e) => {
                let _ = tx.send_blocking(format!("✗ Failed to spawn pkexec: {e}\n"));
            }
            Ok(mut child) => {
                let tx_out = tx.clone();
                let tx_err = tx.clone();
                if let Some(stdout) = child.stdout.take() {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        let _ = tx_out.send_blocking(format!("{line}\n"));
                    }
                }
                if let Some(stderr) = child.stderr.take() {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        let _ = tx_err.send_blocking(format!("{line}\n"));
                    }
                }
                let done = match child.wait() {
                    Ok(s) if s.success() => "\n✓ Done. Please refresh.\n".to_string(),
                    Ok(s)  => format!("\n✗ Failed (exit {})\n", s.code().unwrap_or(-1)),
                    Err(e) => format!("\n✗ wait() error: {e}\n"),
                };
                let _ = tx.send_blocking(done);
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Widget helpers
// ---------------------------------------------------------------------------

fn action_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(if subtitle.is_empty() { "—" } else { subtitle });
    row.set_subtitle_selectable(true);
    row
}

fn pkg_group(title: &str, pkgs: &HashMap<String, String>, empty_msg: &str)
    -> adw::PreferencesGroup
{
    let grp = adw::PreferencesGroup::new();
    grp.set_title(title);
    if pkgs.is_empty() {
        let row = adw::ActionRow::new();
        row.set_title(empty_msg);
        grp.add(&row);
    } else {
        let mut sorted: Vec<_> = pkgs.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        for (name, ver) in sorted {
            grp.add(&action_row(name, ver));
        }
    }
    grp
}

fn install_button_row(title: &str, subtitle: &str, cmd: String,
                      tx: async_channel::Sender<String>,
                      stack: adw::ViewStack) -> adw::ActionRow
{
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_activatable(true);
    row.add_suffix(&gtk4::Image::from_icon_name("system-software-install-symbolic"));
    row.connect_activated(move |_| {
        // Switch to Action Log tab so user can watch progress
        stack.set_visible_child_name("log");
        spawn_pkexec(cmd.clone(), tx.clone());
    });
    row
}

// ---------------------------------------------------------------------------
// GPU detail page
// ---------------------------------------------------------------------------

fn build_gpu_page(gpu: &GpuInfo, free: bool, nonfree: bool,
                  log_tx: async_channel::Sender<String>,
                  stack: adw::ViewStack) -> gtk4::ScrolledWindow
{
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(700);
    clamp.set_margin_top(16);
    clamp.set_margin_bottom(16);
    clamp.set_margin_start(16);
    clamp.set_margin_end(16);

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 24);

    // ── Identity group ───────────────────────────────────────────────────────
    let is_igpu = gpu.vendor == Vendor::Amd && Vendor::is_amd_igpu(&gpu.pci_id)
               || gpu.vendor == Vendor::Intel;
    let gpu_label = if gpu.vendor == Vendor::Amd && Vendor::is_amd_igpu(&gpu.pci_id) {
        format!("{} GPU (Integrated)", gpu.vendor.label())
    } else if gpu.vendor == Vendor::Intel {
        "Intel GPU (Integrated)".to_string()
    } else {
        format!("{} GPU", gpu.vendor.label())
    };

    let id_grp = adw::PreferencesGroup::new();
    id_grp.set_title(&gpu_label);
    id_grp.set_description(Some(gpu.vendor.subtitle()));
    for (lbl, val) in [
        ("PCI Slot",       gpu.slot.as_str()),
        ("PCI ID",         gpu.pci_id.as_str()),
        ("Driver in use",  gpu.driver_in_use.as_str()),
        ("Kernel modules", gpu.driver_modules.as_str()),
    ] {
        id_grp.add(&action_row(lbl, val));
    }
    for (k, v) in &gpu.extra {
        id_grp.add(&action_row(k, v));
    }
    vbox.append(&id_grp);

    // ── Driver action group (vendor-specific) ────────────────────────────────
    match gpu.vendor {
        Vendor::Nvidia => {
            let nvidia_grp = adw::PreferencesGroup::new();
            nvidia_grp.set_title("Driver Management");

            if !nonfree {
                // RPM Fusion nonfree not enabled — must install first
                nvidia_grp.set_description(Some(
                    "RPM Fusion nonfree is required for the NVIDIA proprietary driver."
                ));
                let tx = log_tx.clone();
                let st = stack.clone();
                nvidia_grp.add(&install_button_row(
                    "Install RPM Fusion &amp; NVIDIA Driver",
                    "Installs RPM Fusion nonfree repos then akmod-nvidia",
                    format!("{} && dnf install -y akmod-nvidia xorg-x11-drv-nvidia \
                             xorg-x11-drv-nvidia-power nvidia-settings nvidia-modprobe",
                             install_rpmfusion_cmd()),
                    tx, st,
                ));
            } else if !gpu.installed_pkgs.contains_key("akmod-nvidia") {
                // RPM Fusion enabled but driver not installed
                nvidia_grp.set_description(Some(
                    "NVIDIA proprietary driver is not installed."
                ));
                let tx = log_tx.clone();
                let st = stack.clone();
                nvidia_grp.add(&install_button_row(
                    "Install NVIDIA Driver",
                    "Installs akmod-nvidia, xorg-x11-drv-nvidia, nvidia-settings",
                    "dnf install -y akmod-nvidia xorg-x11-drv-nvidia \
                     xorg-x11-drv-nvidia-power nvidia-settings nvidia-modprobe".to_string(),
                    tx, st,
                ));
            } else {
                // Driver installed — show status
                nvidia_grp.set_description(Some(
                    "NVIDIA proprietary driver is installed. A reboot may be required \
                     after kernel updates for akmod to rebuild."
                ));
                let row = adw::ActionRow::new();
                row.set_title("Driver status");
                let badge = gtk4::Label::new(Some("Installed ✓"));
                badge.add_css_class("success");
                row.add_suffix(&badge);
                nvidia_grp.add(&row);
            }
            vbox.append(&nvidia_grp);
        }

        Vendor::Amd => {
            if !is_igpu {
                // Discrete AMD — note that no action needed
                let amd_grp = adw::PreferencesGroup::new();
                amd_grp.set_title("Driver Status");
                amd_grp.set_description(Some(
                    "AMD GPUs use the open-source amdgpu driver built into the kernel. \
                     No proprietary driver or additional repositories are required."
                ));
                let row = adw::ActionRow::new();
                row.set_title("amdgpu driver");
                let badge = gtk4::Label::new(Some("In-kernel ✓"));
                badge.add_css_class("success");
                row.add_suffix(&badge);
                amd_grp.add(&row);

                // Optional: offer RPM Fusion free for extra Mesa/ROCm packages
                if !free {
                    let tx = log_tx.clone();
                    let st = stack.clone();
                    amd_grp.add(&install_button_row(
                        "Install RPM Fusion (optional)",
                        "Adds extra Mesa builds and ROCm compute packages",
                        install_rpmfusion_cmd(),
                        tx, st,
                    ));
                }
                vbox.append(&amd_grp);
            }
            // iGPU — no action needed, no group shown
        }

        Vendor::Intel => {
            let intel_grp = adw::PreferencesGroup::new();
            intel_grp.set_title("Driver Status");
            intel_grp.set_description(Some(
                "Intel GPUs use the open-source i915/xe driver built into the kernel. \
                 No additional repositories are required for basic operation."
            ));
            let row = adw::ActionRow::new();
            row.set_title("i915/xe driver");
            let badge = gtk4::Label::new(Some("In-kernel ✓"));
            badge.add_css_class("success");
            row.add_suffix(&badge);
            intel_grp.add(&row);
            vbox.append(&intel_grp);
        }

        Vendor::Unknown => {}
    }

    // ── Packages ─────────────────────────────────────────────────────────────
    vbox.append(&pkg_group("Installed Packages", &gpu.installed_pkgs,
                            "No relevant packages installed"));
    vbox.append(&pkg_group("Available via RPM Fusion", &gpu.available_pkgs,
                            "None found — check RPM Fusion repo status"));

    clamp.set_child(Some(&vbox));
    scroll.set_child(Some(&clamp));
    scroll
}

// ---------------------------------------------------------------------------
// Repos page
// ---------------------------------------------------------------------------

fn build_repos_page(
    free: bool,
    nonfree: bool,
    log_tx: async_channel::Sender<String>,
    stack: adw::ViewStack,
) -> gtk4::ScrolledWindow {
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(700);
    clamp.set_margin_top(16);
    clamp.set_margin_bottom(16);
    clamp.set_margin_start(16);
    clamp.set_margin_end(16);

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 24);

    // Status group
    let status_grp = adw::PreferencesGroup::new();
    status_grp.set_title("Repository Status");
    status_grp.set_description(Some(
        "RPM Fusion extends Fedora with proprietary drivers and additional packages. \
         Required for NVIDIA drivers; optional but recommended for AMD and Intel systems.",
    ));
    for (name, desc, enabled) in [
        ("rpmfusion-free",    "Open-source GPU packages (Mesa, ROCm)", free),
        ("rpmfusion-nonfree", "Proprietary GPU drivers (akmod-nvidia)", nonfree),
    ] {
        let row = adw::ActionRow::new();
        row.set_title(name);
        row.set_subtitle(desc);
        let badge = gtk4::Label::new(Some(if enabled { "Enabled ✓" } else { "Disabled" }));
        badge.add_css_class(if enabled { "success" } else { "warning" });
        row.add_suffix(&badge);
        status_grp.add(&row);
    }
    vbox.append(&status_grp);

    // Actions group — smart based on current state
    let actions_grp = adw::PreferencesGroup::new();
    actions_grp.set_title("Actions");

    match (free, nonfree) {
        (true, true) => {
            let row = adw::ActionRow::new();
            row.set_title("RPM Fusion fully configured");
            let badge = gtk4::Label::new(Some("No action needed \u{2713}"));
            badge.add_css_class("success");
            row.add_suffix(&badge);
            actions_grp.add(&row);
        }
        (false, false) => {
            actions_grp.set_description(Some(
                "RPM Fusion is not installed. Click below to install both free and nonfree \
                 repositories and enable the Cisco OpenH264 codec."
            ));
            let tx = log_tx.clone();
            let st = stack.clone();
            actions_grp.add(&install_button_row(
                "Install RPM Fusion (free + nonfree)",
                "Installs both repos and enables fedora-cisco-openh264",
                install_rpmfusion_cmd(),
                tx, st,
            ));
        }
        (false, true) => {
            let ver = run_cmd("rpm -E %fedora");
            let ver = if ver.is_empty() { "$(rpm -E %fedora)".into() } else { ver };
            let tx = log_tx.clone();
            let st = stack.clone();
            actions_grp.add(&install_button_row(
                "Install rpmfusion-free",
                "Adds Mesa, ROCm, and open-source GPU packages",
                format!("dnf install -y https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-{ver}.noarch.rpm"),
                tx, st,
            ));
        }
        (true, false) => {
            let ver = run_cmd("rpm -E %fedora");
            let ver = if ver.is_empty() { "$(rpm -E %fedora)".into() } else { ver };
            let tx = log_tx.clone();
            let st = stack.clone();
            actions_grp.add(&install_button_row(
                "Install rpmfusion-nonfree",
                "Adds akmod-nvidia and proprietary NVIDIA drivers",
                format!("dnf install -y https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-{ver}.noarch.rpm"),
                tx, st,
            ));
        }
    }

    // Legacy note
    let note = gtk4::Label::new(Some(
        "Note: Legacy NVIDIA drivers (390xx, 470xx) and CUDA/ROCm support \
         are not managed by this version of GPU Driver Manager.",
    ));
    note.add_css_class("dim-label");
    note.set_wrap(true);
    note.set_halign(gtk4::Align::Start);
    vbox.append(&note);

    let auth_note = gtk4::Label::new(Some(
        "All install actions require authentication via pkexec.",
    ));
    auth_note.add_css_class("dim-label");
    auth_note.set_wrap(true);
    auth_note.set_halign(gtk4::Align::Start);
    vbox.append(&auth_note);

    clamp.set_child(Some(&vbox));
    scroll.set_child(Some(&clamp));
    scroll
}

// ---------------------------------------------------------------------------
// Action log page
// ---------------------------------------------------------------------------

fn build_log_page(buffer: &gtk4::TextBuffer) -> gtk4::ScrolledWindow {
    let sw = gtk4::ScrolledWindow::new();
    sw.set_vexpand(true);
    sw.set_hscrollbar_policy(gtk4::PolicyType::Automatic);
    let tv = gtk4::TextView::new();
    tv.set_editable(false);
    tv.set_monospace(true);
    tv.set_cursor_visible(false);
    tv.set_left_margin(8);
    tv.set_right_margin(8);
    tv.set_top_margin(8);
    tv.set_buffer(Some(buffer));
    sw.set_child(Some(&tv));
    sw
}

// ---------------------------------------------------------------------------
// Main window
// ---------------------------------------------------------------------------

fn build_window(app: &adw::Application) {
    let win = adw::ApplicationWindow::new(app);
    win.set_title(Some(APP_NAME));
    win.set_default_size(780, 700);

    let log_buffer = gtk4::TextBuffer::new(None);
    let (log_tx, log_rx) = async_channel::unbounded::<String>();
    let (scan_tx, scan_rx) = async_channel::unbounded::<(Vec<GpuInfo>, bool, bool)>();

    // Header
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(APP_NAME, APP_VERSION)));
    let refresh_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Refresh"));
    header.pack_end(&refresh_btn);

    // Stack
    let stack = adw::ViewStack::new();

    // Scanning placeholder
    let placeholder = adw::StatusPage::new();
    placeholder.set_icon_name(Some("computer-symbolic"));
    placeholder.set_title("Scanning…");
    placeholder.set_description(Some("Detecting GPUs and querying package database"));
    stack.add_titled(&placeholder, Some("scanning"), "GPUs");

    // Initial repos page
    let repos_page = build_repos_page(false, false, log_tx.clone(), stack.clone());
    stack.add_titled(&repos_page, Some("repos"), "Repos");

    // Log page
    let log_page = build_log_page(&log_buffer);
    stack.add_titled(&log_page, Some("log"), "Action Log");

    // ViewSwitcherBar
    let switcher = adw::ViewSwitcherBar::new();
    switcher.set_stack(Some(&stack));
    switcher.set_reveal(true);

    // Banner
    let banner = adw::Banner::new("One or more RPM Fusion repositories are not enabled");
    banner.set_button_label(Some("Go to Repos"));
    {
        let stack2 = stack.clone();
        banner.connect_button_clicked(move |_| {
            stack2.set_visible_child_name("repos");
        });
    }

    // Layout
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&stack);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.add_bottom_bar(&switcher);
    toolbar_view.set_content(Some(&content));

    win.set_content(Some(&toolbar_view));
    win.present();

    // Pump log lines into TextBuffer
    {
        let buf = log_buffer.clone();
        gtk4::glib::MainContext::default().spawn_local(async move {
            while let Ok(line) = log_rx.recv().await {
                buf.insert(&mut buf.end_iter(), &line);
            }
        });
    }

    // Background scan
    thread::spawn(move || {
        let mut gpus = detect_gpus();
        for gpu in &mut gpus {
            enrich_gpu(gpu);
        }
        let (free, nonfree) = rpmfusion_enabled();
        let _ = scan_tx.send_blocking((gpus, free, nonfree));
    });

    // Handle scan result
    let stack_r  = stack.clone();
    let banner_r = banner.clone();
    let log_tx_r = log_tx.clone();

    gtk4::glib::MainContext::default().spawn_local(async move {
        if let Ok((gpus, free, nonfree)) = scan_rx.recv().await {
            if let Some(child) = stack_r.child_by_name("scanning") {
                stack_r.remove(&child);
            }

            for (i, gpu) in gpus.iter().enumerate() {
                let page = build_gpu_page(gpu, free, nonfree,
                                          log_tx_r.clone(), stack_r.clone());
                let id    = format!("gpu{i}");
                let is_igpu = gpu.vendor == Vendor::Amd && Vendor::is_amd_igpu(&gpu.pci_id);
                let title = if gpus.len() == 1 {
                    format!("{} GPU", gpu.vendor.label())
                } else if is_igpu {
                    format!("{} iGPU", gpu.vendor.label())
                } else if gpu.vendor == Vendor::Intel {
                    "Intel iGPU".to_string()
                } else {
                    format!("{} #{}", gpu.vendor.label(), i + 1)
                };
                stack_r.add_titled(&page, Some(id.as_str()), &title);
            }

            if !gpus.is_empty() {
                stack_r.set_visible_child_name("gpu0");
            }

            if let Some(old) = stack_r.child_by_name("repos") {
                stack_r.remove(&old);
            }
            let new_repos = build_repos_page(free, nonfree, log_tx_r, stack_r.clone());
            stack_r.add_titled(&new_repos, Some("repos"), "Repos");

            banner_r.set_revealed(!free || !nonfree);
        }
    });

    // Refresh
    let app2 = app.clone();
    refresh_btn.connect_clicked(move |_| {
        for w in app2.windows() { w.close(); }
        app2.activate();
    });
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let app = adw::Application::new(Some(APP_ID), gtk4::gio::ApplicationFlags::empty());
    app.connect_activate(build_window);
    app.run();
}
