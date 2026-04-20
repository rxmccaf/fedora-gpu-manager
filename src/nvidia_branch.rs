// NVIDIA driver branch detection
// Generated from RPM Fusion supported chips lists:
//   xorg-x11-drv-nvidia-580.126.18 (current)
//   xorg-x11-drv-nvidia-470xx-470.256.02 (legacy)
//   xorg-x11-drv-nvidia-390xx-390.157 (legacy)
//
// Logic: check 390xx-only list first, then 470xx-only list,
// anything else gets the current driver.

pub fn nvidia_driver_branch(pci_device_id: &str) -> &'static str {
    let id = pci_device_id.to_uppercase();
    let id = id.trim_start_matches("0X"); // handle both "0FC0" and "0x0FC0"

    // 57 device IDs supported ONLY by 390xx (Kepler architecture)
    // GTX 600/700 series and some mobile variants
    const NEEDS_390XX: &[&str] = &[
        "0FCD", "0FCE", "0FD1", "0FD2", "0FD3", "0FD4", "0FD5",
        "0FD8", "0FD9", "0FDF", "0FE0", "0FE1", "0FE2", "0FE3",
        "0FE4", "0FE9", "0FEA", "0FEC", "0FED", "0FEE", "0FF6",
        "0FF8", "0FFB", "0FFC", "1198", "1199", "119A", "119D",
        "119E", "119F", "11A0", "11A1", "11A2", "11A3", "11A7",
        "11B6", "11B7", "11B8", "11BC", "11BD", "11BE", "11E0",
        "11E1", "11E2", "11E3", "11FC", "1290", "1291", "1292",
        "1293", "1295", "1296", "1298", "1299", "129A", "12B9",
        "12BA",
    ];

    // 8 device IDs supported ONLY by 470xx (Maxwell/Pascal era)
    const NEEDS_470XX: &[&str] = &[
        "0FC0", "0FC1", "0FC2", "0FF3",
        "1BB3", "1DF5", "1EB8", "1F09",
    ];

    if NEEDS_390XX.contains(&id) {
        "390xx"
    } else if NEEDS_470XX.contains(&id) {
        "470xx"
    } else {
        "current"
    }
}

// Returns the correct package set for a given driver branch
pub fn nvidia_packages_for_branch(branch: &str) -> (&'static str, &'static str) {
    match branch {
        "390xx" => (
            "akmod-nvidia-390xx xorg-x11-drv-nvidia-390xx nvidia-settings-390xx",
            "390xx legacy driver (GTX 600/700 series)"
        ),
        "470xx" => (
            "akmod-nvidia-470xx xorg-x11-drv-nvidia-470xx nvidia-settings-470xx",
            "470xx legacy driver"
        ),
        _ => (
            "akmod-nvidia xorg-x11-drv-nvidia xorg-x11-drv-nvidia-power \
             nvidia-settings nvidia-modprobe",
            "current driver (Turing/Ampere/Ada/Blackwell)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rtx_3060_mobile_is_current() {
        // 10de:25a0 — RTX 3060 Mobile (Ampere)
        assert_eq!(nvidia_driver_branch("25A0"), "current");
    }

    #[test]
    fn test_rtx_2080_super_is_current() {
        // 10de:1e81 — RTX 2080 Super (Turing)
        assert_eq!(nvidia_driver_branch("1E81"), "current");
    }

    #[test]
    fn test_gtx_680_is_390xx() {
        // 10de:1198 — GTX 680 (Kepler)
        assert_eq!(nvidia_driver_branch("1198"), "390xx");
    }

    #[test]
    fn test_case_insensitive() {
        assert_eq!(nvidia_driver_branch("1198"), "390xx");
        assert_eq!(nvidia_driver_branch("0fc0"), "470xx");
    }

    #[test]
    fn test_470xx_ids() {
        for id in ["0FC0", "0FC1", "0FC2", "0FF3", "1BB3", "1DF5", "1EB8", "1F09"] {
            assert_eq!(nvidia_driver_branch(id), "470xx", "Failed for {}", id);
        }
    }
}
