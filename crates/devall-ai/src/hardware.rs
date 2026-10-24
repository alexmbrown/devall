//! Hardware detection for model recommendations and the llama.cpp build choice.
//!
//! * macOS: `sysctl` (RAM, CPU, `hw.optional.arm64` — true on Apple Silicon even under Rosetta)
//!   and `system_profiler SPDisplaysDataType -json` (GPU names).
//! * Windows: one PowerShell call (`Get-CimInstance Win32_ComputerSystem/Win32_Processor/
//!   Win32_VideoController` plus the display-class registry's 64-bit `qwMemorySize`, since
//!   `AdapterRAM` is capped at 4 GB), falling back to `wmic` on systems without PowerShell.
//! * NVIDIA everywhere: `nvidia-smi --query-gpu=name,memory.total,driver_version`.
//!
//! Detection runs only when asked (it spawns processes) — call it from a blocking task.

use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Qualcomm,
    Other,
}

impl GpuVendor {
    pub fn from_name(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.contains("nvidia") || n.contains("geforce") || n.contains("quadro") || n.contains("rtx") {
            GpuVendor::Nvidia
        } else if n.contains("amd") || n.contains("radeon") {
            GpuVendor::Amd
        } else if n.contains("intel") || n.contains("arc ") || n.contains("iris") || n.contains("uhd") {
            GpuVendor::Intel
        } else if n.contains("apple") {
            GpuVendor::Apple
        } else if n.contains("qualcomm") || n.contains("adreno") {
            GpuVendor::Qualcomm
        } else {
            GpuVendor::Other
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    pub vendor: GpuVendor,
    /// Dedicated memory when known (unified-memory GPUs report `None`).
    pub vram_bytes: Option<u64>,
    pub driver_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    /// `macos`, `windows`, `linux`.
    pub os: String,
    /// `aarch64`, `x86_64`.
    pub arch: String,
    pub total_ram_bytes: u64,
    pub cpu: Option<String>,
    pub apple_silicon: bool,
    pub gpus: Vec<Gpu>,
    /// `nvidia-smi` answered (NVIDIA driver installed).
    pub nvidia_smi: bool,
    pub notes: Vec<String>,
}

impl Hardware {
    pub fn total_ram_gib(&self) -> f64 {
        self.total_ram_bytes as f64 / GIB as f64
    }

    /// The NVIDIA GPU with the most VRAM, if any.
    pub fn best_nvidia(&self) -> Option<&Gpu> {
        self.gpus.iter().filter(|g| g.vendor == GpuVendor::Nvidia).max_by_key(|g| g.vram_bytes.unwrap_or(0))
    }

    /// The discrete GPU with the most known VRAM (NVIDIA/AMD/Intel Arc).
    pub fn best_dgpu(&self) -> Option<&Gpu> {
        self.gpus
            .iter()
            .filter(|g| matches!(g.vendor, GpuVendor::Nvidia | GpuVendor::Amd | GpuVendor::Intel) && g.vram_bytes.is_some_and(|v| v >= 4 * GIB))
            .max_by_key(|g| g.vram_bytes.unwrap_or(0))
    }
}

/// Hardware tiers from the Oct 2026 lineup (see `research_notes/…/local_models_update_oct2026.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    /// ~8 GB RAM (8 GB M1/M2, 8 GB laptops, Intel Macs, CPU/iGPU).
    T8,
    /// 16 GB (base M-series, 16 GB laptops, 6–8 GB VRAM).
    T16,
    /// 24 GB unified or 12–16 GB VRAM.
    T24,
    /// 32 GB+ unified or 24 GB+ VRAM.
    T32,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::T8 => "~8 GB",
            Tier::T16 => "16 GB",
            Tier::T24 => "24 GB",
            Tier::T32 => "32 GB+",
        }
    }
}

fn tier_for_gib(gib: f64) -> Tier {
    // Thresholds sit between nominal sizes (a "16 GB" machine reports ~15.7 GiB).
    if gib < 12.0 {
        Tier::T8
    } else if gib < 20.0 {
        Tier::T16
    } else if gib < 28.0 {
        Tier::T24
    } else {
        Tier::T32
    }
}

/// Picks the tier: unified memory on Apple Silicon; VRAM on discrete GPUs (with RAM as a floor
/// up to 16 GB-class); CPU-only machines are capped at the 16 GB tier because 27B dense models
/// are too slow without a GPU.
pub fn tier(hw: &Hardware) -> Tier {
    let ram = tier_for_gib(hw.total_ram_gib());
    if hw.apple_silicon {
        return ram;
    }
    match hw.best_dgpu().and_then(|g| g.vram_bytes) {
        Some(vram) => {
            let v = vram as f64 / GIB as f64;
            let vram_tier = if v < 6.0 {
                Tier::T8
            } else if v < 11.0 {
                Tier::T16
            } else if v < 20.0 {
                Tier::T24
            } else {
                Tier::T32
            };
            vram_tier.max(ram.min(Tier::T16))
        }
        None => ram.min(Tier::T16),
    }
}

/// Which prebuilt llama.cpp binary fits this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlamaVariant {
    MacosArm64,
    MacosX64,
    WinCpuX64,
    WinVulkanX64,
    WinCuda124X64,
    WinCuda134X64,
    WinCpuArm64,
    WinVulkanArm64,
    UbuntuX64,
    UbuntuVulkanX64,
    UbuntuArm64,
}

impl LlamaVariant {
    /// Suffix used in llama.cpp release asset names (`llama-<build>-bin-<suffix>.<ext>`).
    pub fn asset_suffix(self) -> &'static str {
        match self {
            LlamaVariant::MacosArm64 => "macos-arm64",
            LlamaVariant::MacosX64 => "macos-x64",
            LlamaVariant::WinCpuX64 => "win-cpu-x64",
            LlamaVariant::WinVulkanX64 => "win-vulkan-x64",
            LlamaVariant::WinCuda124X64 => "win-cuda-12.4-x64",
            LlamaVariant::WinCuda134X64 => "win-cuda-13.4-x64",
            LlamaVariant::WinCpuArm64 => "win-cpu-arm64",
            LlamaVariant::WinVulkanArm64 => "win-vulkan-arm64",
            LlamaVariant::UbuntuX64 => "ubuntu-x64",
            LlamaVariant::UbuntuVulkanX64 => "ubuntu-vulkan-x64",
            LlamaVariant::UbuntuArm64 => "ubuntu-arm64",
        }
    }

    /// Whether the build offloads to a GPU (`-ngl` makes sense).
    pub fn gpu(self) -> bool {
        !matches!(self, LlamaVariant::WinCpuX64 | LlamaVariant::WinCpuArm64 | LlamaVariant::UbuntuX64 | LlamaVariant::UbuntuArm64 | LlamaVariant::MacosX64)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantChoice {
    pub variant: LlamaVariant,
    /// Also usable, in order of preference (e.g. Vulkan, then CPU when CUDA fails to load).
    pub fallbacks: Vec<LlamaVariant>,
    pub reason: String,
}

fn driver_major(v: &str) -> Option<u32> {
    v.split('.').next()?.trim().parse().ok()
}

/// Chooses the llama.cpp build: Metal on Apple Silicon, CPU on Intel Macs; on Windows CUDA 13.4
/// for NVIDIA drivers ≥ 580 (12.4 for ≥ 551), Vulkan for other GPUs, CPU otherwise.
pub fn llama_variant(hw: &Hardware) -> VariantChoice {
    let arm = hw.arch == "aarch64";
    let has_gpu = hw.gpus.iter().any(|g| g.vendor != GpuVendor::Other);
    match hw.os.as_str() {
        "macos" if hw.apple_silicon => VariantChoice { variant: LlamaVariant::MacosArm64, fallbacks: vec![], reason: "Apple Silicon: Metal build".into() },
        "macos" => VariantChoice { variant: LlamaVariant::MacosX64, fallbacks: vec![], reason: "Intel Mac: CPU build".into() },
        "windows" if arm => {
            if has_gpu {
                VariantChoice { variant: LlamaVariant::WinVulkanArm64, fallbacks: vec![LlamaVariant::WinCpuArm64], reason: "Windows on Arm with a GPU: Vulkan build".into() }
            } else {
                VariantChoice { variant: LlamaVariant::WinCpuArm64, fallbacks: vec![], reason: "Windows on Arm: CPU build".into() }
            }
        }
        "windows" => {
            if let Some(nv) = hw.best_nvidia().filter(|_| hw.nvidia_smi) {
                let major = nv.driver_version.as_deref().and_then(driver_major).unwrap_or(0);
                if major >= 580 {
                    return VariantChoice {
                        variant: LlamaVariant::WinCuda134X64,
                        fallbacks: vec![LlamaVariant::WinVulkanX64, LlamaVariant::WinCpuX64],
                        reason: format!("NVIDIA {} (driver {}): CUDA 13.4 build (~580 MB download incl. runtime)", nv.name, nv.driver_version.as_deref().unwrap_or("?")),
                    };
                }
                if major >= 551 {
                    return VariantChoice {
                        variant: LlamaVariant::WinCuda124X64,
                        fallbacks: vec![LlamaVariant::WinVulkanX64, LlamaVariant::WinCpuX64],
                        reason: format!("NVIDIA {} (driver {major}): CUDA 12.4 build — update the driver to ≥ 580 for CUDA 13", nv.name),
                    };
                }
            }
            if has_gpu {
                VariantChoice { variant: LlamaVariant::WinVulkanX64, fallbacks: vec![LlamaVariant::WinCpuX64], reason: "GPU detected: Vulkan build (AMD, Intel and NVIDIA)".into() }
            } else {
                VariantChoice { variant: LlamaVariant::WinCpuX64, fallbacks: vec![], reason: "No usable GPU: CPU build".into() }
            }
        }
        _ if arm => VariantChoice { variant: LlamaVariant::UbuntuArm64, fallbacks: vec![], reason: "Linux arm64: CPU build".into() },
        _ if has_gpu => VariantChoice { variant: LlamaVariant::UbuntuVulkanX64, fallbacks: vec![LlamaVariant::UbuntuX64], reason: "Linux with a GPU: Vulkan build".into() },
        _ => VariantChoice { variant: LlamaVariant::UbuntuX64, fallbacks: vec![], reason: "Linux: CPU build".into() },
    }
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    let out = c.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parses `nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits`.
pub fn parse_nvidia_smi(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|l| {
            let parts: Vec<&str> = l.split(',').map(str::trim).collect();
            if parts.len() < 3 || parts[0].is_empty() {
                return None;
            }
            Some(Gpu {
                name: parts[0].to_string(),
                vendor: GpuVendor::Nvidia,
                vram_bytes: parts[1].parse::<u64>().ok().map(|mib| mib * 1024 * 1024),
                driver_version: Some(parts[2].to_string()),
            })
        })
        .collect()
}

const WINDOWS_PROBE: &str = r#"$ErrorActionPreference='SilentlyContinue'
$cs = Get-CimInstance Win32_ComputerSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$gpus = @(Get-CimInstance Win32_VideoController | ForEach-Object { [pscustomobject]@{ name = $_.Name; ram = [uint64]$_.AdapterRAM; driver = $_.DriverVersion } })
$reg = @(Get-ItemProperty 'HKLM:\SYSTEM\ControlSet001\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*' | ForEach-Object { try { [pscustomobject]@{ name = $_.DriverDesc; mem = [uint64]$_.'HardwareInformation.qwMemorySize' } } catch {} })
[pscustomobject]@{ totalRam = [uint64]$cs.TotalPhysicalMemory; cpu = $cpu.Name; gpus = $gpus; reg = $reg } | ConvertTo-Json -Compress -Depth 4"#;

fn as_list(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Null => vec![],
        other => vec![other.clone()],
    }
}

/// Parses the PowerShell probe output into (RAM, CPU, GPUs).
pub fn parse_windows_probe(json: &str) -> Option<(u64, Option<String>, Vec<Gpu>)> {
    let v: Value = serde_json::from_str(json.trim().trim_start_matches('\u{feff}')).ok()?;
    let ram = v["totalRam"].as_u64().unwrap_or(0);
    let cpu = v["cpu"].as_str().map(|s| s.trim().to_string());
    let reg = as_list(&v["reg"]);
    let gpus = as_list(&v["gpus"])
        .iter()
        .filter_map(|g| {
            let name = g["name"].as_str()?.trim().to_string();
            let reg_mem = reg.iter().find(|r| r["name"].as_str().map(str::trim) == Some(name.as_str())).and_then(|r| r["mem"].as_u64()).filter(|m| *m > 0);
            let vram = reg_mem.or_else(|| g["ram"].as_u64().filter(|m| *m > 0));
            Some(Gpu { vendor: GpuVendor::from_name(&name), name, vram_bytes: vram, driver_version: g["driver"].as_str().map(str::to_string) })
        })
        .filter(|g| !g.name.to_ascii_lowercase().contains("basic display") && !g.name.to_ascii_lowercase().contains("remote display"))
        .collect();
    Some((ram, cpu, gpus))
}

/// Detects this machine. Blocking (spawns `sysctl`, PowerShell, `nvidia-smi`).
pub fn detect() -> Hardware {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        other => other,
    }
    .to_string();
    let mut hw = Hardware {
        os: os.clone(),
        arch: std::env::consts::ARCH.to_string(),
        total_ram_bytes: 0,
        cpu: None,
        apple_silicon: false,
        gpus: Vec::new(),
        nvidia_smi: false,
        notes: Vec::new(),
    };
    match os.as_str() {
        "macos" => {
            hw.total_ram_bytes = run("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.trim().parse().ok()).unwrap_or(0);
            hw.cpu = run("sysctl", &["-n", "machdep.cpu.brand_string"]).map(|s| s.trim().to_string());
            hw.apple_silicon = run("sysctl", &["-n", "hw.optional.arm64"]).is_some_and(|s| s.trim() == "1");
            if hw.apple_silicon {
                hw.arch = "aarch64".into();
                if std::env::consts::ARCH != "aarch64" {
                    hw.notes.push("/dev/all is running under Rosetta; the native Apple Silicon build would be faster.".into());
                }
            }
            if let Some(json) = run("system_profiler", &["SPDisplaysDataType", "-json"])
                && let Ok(v) = serde_json::from_str::<Value>(&json)
            {
                for d in as_list(&v["SPDisplaysDataType"]) {
                    if let Some(name) = d["sppci_model"].as_str() {
                        let vram = d["spdisplays_vram"].as_str().and_then(|s| s.split_whitespace().next()?.parse::<u64>().ok()).map(|n| n * GIB);
                        hw.gpus.push(Gpu { name: name.to_string(), vendor: GpuVendor::from_name(name), vram_bytes: vram, driver_version: None });
                    }
                }
            }
        }
        "windows" => {
            let probe = run("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", WINDOWS_PROBE]);
            match probe.as_deref().and_then(parse_windows_probe) {
                Some((ram, cpu, gpus)) => {
                    hw.total_ram_bytes = ram;
                    hw.cpu = cpu;
                    hw.gpus = gpus;
                }
                None => {
                    hw.notes.push("PowerShell hardware probe failed; used wmic.".into());
                    hw.total_ram_bytes = run("wmic", &["ComputerSystem", "get", "TotalPhysicalMemory", "/value"])
                        .and_then(|s| s.lines().find_map(|l| l.trim().strip_prefix("TotalPhysicalMemory=").and_then(|n| n.trim().parse().ok())))
                        .unwrap_or(0);
                    if let Some(s) = run("wmic", &["path", "Win32_VideoController", "get", "Name", "/value"]) {
                        for name in s.lines().filter_map(|l| l.trim().strip_prefix("Name=")).filter(|n| !n.is_empty()) {
                            hw.gpus.push(Gpu { name: name.to_string(), vendor: GpuVendor::from_name(name), vram_bytes: None, driver_version: None });
                        }
                    }
                }
            }
        }
        _ => {
            if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
                hw.total_ram_bytes = meminfo
                    .lines()
                    .find_map(|l| l.strip_prefix("MemTotal:").and_then(|r| r.split_whitespace().next()?.parse::<u64>().ok()))
                    .map(|kb| kb * 1024)
                    .unwrap_or(0);
            }
            hw.cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|c| c.lines().find_map(|l| l.strip_prefix("model name").map(|r| r.trim_start_matches([' ', '\t', ':']).to_string())));
        }
    }
    if let Some(out) = run("nvidia-smi", &["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"]) {
        hw.nvidia_smi = true;
        let nv = parse_nvidia_smi(&out);
        // nvidia-smi knows the real VRAM; replace the generic entries for the same cards.
        hw.gpus.retain(|g| g.vendor != GpuVendor::Nvidia);
        hw.gpus.extend(nv);
    }
    if hw.total_ram_bytes == 0 {
        hw.notes.push("Could not read the installed memory; assuming the smallest tier.".into());
    }
    hw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(os: &str, arch: &str, ram_gib: u64, apple: bool, gpus: Vec<Gpu>) -> Hardware {
        Hardware {
            os: os.into(),
            arch: arch.into(),
            total_ram_bytes: ram_gib * GIB - 300 * 1024 * 1024,
            cpu: None,
            apple_silicon: apple,
            nvidia_smi: gpus.iter().any(|g| g.vendor == GpuVendor::Nvidia),
            gpus,
            notes: vec![],
        }
    }

    fn nv(vram_gib: u64, driver: &str) -> Gpu {
        Gpu { name: "NVIDIA GeForce RTX".into(), vendor: GpuVendor::Nvidia, vram_bytes: Some(vram_gib * GIB), driver_version: Some(driver.into()) }
    }

    #[test]
    fn tiers() {
        assert_eq!(tier(&hw("macos", "aarch64", 8, true, vec![])), Tier::T8);
        assert_eq!(tier(&hw("macos", "aarch64", 16, true, vec![])), Tier::T16);
        assert_eq!(tier(&hw("macos", "aarch64", 24, true, vec![])), Tier::T24);
        assert_eq!(tier(&hw("macos", "aarch64", 64, true, vec![])), Tier::T32);
        // Intel Mac / CPU-only boxes never go above the 16 GB tier.
        assert_eq!(tier(&hw("macos", "x86_64", 32, false, vec![])), Tier::T16);
        assert_eq!(tier(&hw("windows", "x86_64", 64, false, vec![])), Tier::T16);
        // VRAM drives discrete-GPU machines.
        assert_eq!(tier(&hw("windows", "x86_64", 16, false, vec![nv(8, "581.15")])), Tier::T16);
        assert_eq!(tier(&hw("windows", "x86_64", 32, false, vec![nv(12, "581.15")])), Tier::T24);
        assert_eq!(tier(&hw("windows", "x86_64", 32, false, vec![nv(24, "581.15")])), Tier::T32);
        // A small GPU doesn't drag a 16 GB machine below its RAM tier.
        assert_eq!(tier(&hw("windows", "x86_64", 16, false, vec![nv(4, "581.15")])), Tier::T16);
        assert_eq!(tier(&hw("windows", "x86_64", 8, false, vec![])), Tier::T8);
    }

    #[test]
    fn llama_builds() {
        assert_eq!(llama_variant(&hw("macos", "aarch64", 16, true, vec![])).variant, LlamaVariant::MacosArm64);
        assert_eq!(llama_variant(&hw("macos", "x86_64", 16, false, vec![])).variant, LlamaVariant::MacosX64);
        assert_eq!(llama_variant(&hw("windows", "x86_64", 32, false, vec![nv(24, "581.15")])).variant, LlamaVariant::WinCuda134X64);
        assert_eq!(llama_variant(&hw("windows", "x86_64", 32, false, vec![nv(24, "552.44")])).variant, LlamaVariant::WinCuda124X64);
        let old = llama_variant(&hw("windows", "x86_64", 32, false, vec![nv(24, "531.10")]));
        assert_eq!(old.variant, LlamaVariant::WinVulkanX64);
        let amd = Gpu { name: "AMD Radeon RX 7800 XT".into(), vendor: GpuVendor::Amd, vram_bytes: Some(16 * GIB), driver_version: None };
        assert_eq!(llama_variant(&hw("windows", "x86_64", 32, false, vec![amd])).variant, LlamaVariant::WinVulkanX64);
        assert_eq!(llama_variant(&hw("windows", "x86_64", 16, false, vec![])).variant, LlamaVariant::WinCpuX64);
        let adreno = Gpu { name: "Qualcomm(R) Adreno(TM) X1-85 GPU".into(), vendor: GpuVendor::Qualcomm, vram_bytes: None, driver_version: None };
        assert_eq!(llama_variant(&hw("windows", "aarch64", 16, false, vec![adreno])).variant, LlamaVariant::WinVulkanArm64);
        assert!(LlamaVariant::MacosArm64.gpu() && !LlamaVariant::WinCpuX64.gpu());
    }

    #[test]
    fn parses_probes() {
        let gpus = parse_nvidia_smi("NVIDIA GeForce RTX 4090, 24564, 581.15\n");
        assert_eq!(gpus[0].vram_bytes, Some(24564 * 1024 * 1024));
        assert_eq!(gpus[0].driver_version.as_deref(), Some("581.15"));
        let json = r#"{"totalRam":34359738368,"cpu":"AMD Ryzen 9 7950X ","gpus":[{"name":"AMD Radeon RX 7900 XTX","ram":4293918720,"driver":"32.0.21013.1000"},{"name":"Microsoft Basic Display Adapter","ram":0,"driver":"10"}],"reg":{"name":"AMD Radeon RX 7900 XTX","mem":25753026560}}"#;
        let (ram, cpu, gpus) = parse_windows_probe(json).unwrap();
        assert_eq!(ram, 34359738368);
        assert_eq!(cpu.as_deref(), Some("AMD Ryzen 9 7950X"));
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].vendor, GpuVendor::Amd);
        assert_eq!(gpus[0].vram_bytes, Some(25753026560), "64-bit registry value wins over the 4 GB AdapterRAM cap");
    }

    #[test]
    fn detects_this_machine() {
        let hw = detect();
        assert!(!hw.os.is_empty());
        // Tier selection never panics on real data.
        let _ = tier(&hw);
        let _ = llama_variant(&hw);
    }
}
