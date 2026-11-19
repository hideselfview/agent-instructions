use super::*;
use crate::testing::Scratch;
use std::os::unix::fs::PermissionsExt;

#[test]
fn reads_the_lines_the_script_prints_and_fails_on_anything_else() {
    let memory =
        Memory::parse("memory_free_percent=61\nswap_used_mib=12.50\nmemory_pressure_level=2\n")
            .unwrap();
    assert_eq!(
        memory,
        Memory {
            free_percent: 61,
            swap_used_mib: 12.5,
            pressure: Pressure::Warning,
        }
    );
    assert!(memory.under_pressure());
    assert_eq!(
        memory.text(),
        "memory pressure warning, 61% free, swap 12M used"
    );
    assert!(!Memory::at_rest().under_pressure());
    let swapping = Memory {
        swap_used_mib: 2400.0,
        ..Memory::at_rest()
    };
    assert!(!swapping.under_pressure(), "swap alone is not pressure");
    assert!(
        Memory {
            pressure: Pressure::Critical,
            ..swapping
        }
        .under_pressure()
    );
    for text in [
        "swap_used_mib=0\nmemory_pressure_level=1\n",
        "memory_free_percent=61\nmemory_pressure_level=1\n",
        "memory_free_percent=61\nswap_used_mib=0\n",
        "memory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=\n",
        "memory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=3\n",
        "memory_free_percent=\nswap_used_mib=0\nmemory_pressure_level=1\n",
        "memory_free_percent=61\nswap_used_mib=\nmemory_pressure_level=1\n",
    ] {
        assert!(Memory::parse(text).is_err(), "{text:?}");
    }
}

/// The script turns the unit of `vm.swapusage` into MiB, and reads the free
/// percentage `memory_pressure` prints and the kernel's pressure level.
#[test]
fn the_script_reads_memory_pressure_and_swap_in_any_unit() {
    let scratch = Scratch::new("host-memory");
    let bin = scratch.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = |name: &str, body: &str| {
        let path = bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    program(
        "memory_pressure",
        "echo 'The system has 17179869184 (1048576 pages)'; echo 'System-wide memory free percentage: 37%'",
    );
    for (swap, mib) in [
        (
            "total = 3072.00M  used = 1024.50M  free = 2047.50M  (encrypted)",
            1024.5,
        ),
        (
            "total = 4.00G  used = 2.00G  free = 2.00G  (encrypted)",
            2048.0,
        ),
        (
            "total = 0.00M  used = 0.00M  free = 0.00M  (encrypted)",
            0.0,
        ),
        (
            "total = 512.00K  used = 512.00K  free = 0.00K  (encrypted)",
            0.5,
        ),
    ] {
        program(
            "sysctl",
            &format!("case \"$2\" in kern.*) echo 4;; *) echo '{swap}';; esac"),
        );
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("os=Darwin\n{SCRIPT}"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        let memory = Memory::parse(&String::from_utf8_lossy(&output.stdout)).unwrap();
        assert_eq!(memory.free_percent, 37);
        assert_eq!(memory.pressure, Pressure::Critical);
        assert_eq!(memory.swap_used_mib, mib, "{swap}");
    }
}

/// This Mac answers the script.
#[test]
fn this_mac_reports_its_memory() {
    let memory = here().unwrap();
    assert!(memory.free_percent <= 100);
}

/// A pressure level the kernel does not define fails the probe and names the
/// value, so health says the host's answer cannot be read.
#[test]
fn an_unknown_pressure_level_fails_naming_the_value() {
    let fault = Memory::parse("memory_free_percent=61\nswap_used_mib=0\nmemory_pressure_level=3\n")
        .unwrap_err()
        .text();
    assert!(fault.contains("\"3\""), "{fault}");
}
