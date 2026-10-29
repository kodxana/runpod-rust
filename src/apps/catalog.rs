//! Pinned Python catalog values; regenerate with tools/catalog_types.py.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Upstream GpuGroup catalog values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum GpuGroup {
    /// any.
    #[serde(rename = "any")]
    Any,
    /// ADA_24.
    #[serde(rename = "ADA_24")]
    Ada24,
    /// ADA_32_PRO.
    #[serde(rename = "ADA_32_PRO")]
    Ada32Pro,
    /// ADA_48_PRO.
    #[serde(rename = "ADA_48_PRO")]
    Ada48Pro,
    /// ADA_80_PRO.
    #[serde(rename = "ADA_80_PRO")]
    Ada80Pro,
    /// AMPERE_16.
    #[serde(rename = "AMPERE_16")]
    Ampere16,
    /// AMPERE_24.
    #[serde(rename = "AMPERE_24")]
    Ampere24,
    /// AMPERE_48.
    #[serde(rename = "AMPERE_48")]
    Ampere48,
    /// AMPERE_80.
    #[serde(rename = "AMPERE_80")]
    Ampere80,
    /// HOPPER_141.
    #[serde(rename = "HOPPER_141")]
    Hopper141,
    /// BLACKWELL_96.
    #[serde(rename = "BLACKWELL_96")]
    Blackwell96,
    /// BLACKWELL_180.
    #[serde(rename = "BLACKWELL_180")]
    Blackwell180,
}
impl GpuGroup {
    /// Returns the platform's exact identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Ada24 => "ADA_24",
            Self::Ada32Pro => "ADA_32_PRO",
            Self::Ada48Pro => "ADA_48_PRO",
            Self::Ada80Pro => "ADA_80_PRO",
            Self::Ampere16 => "AMPERE_16",
            Self::Ampere24 => "AMPERE_24",
            Self::Ampere48 => "AMPERE_48",
            Self::Ampere80 => "AMPERE_80",
            Self::Hopper141 => "HOPPER_141",
            Self::Blackwell96 => "BLACKWELL_96",
            Self::Blackwell180 => "BLACKWELL_180",
        }
    }
    /// Returns concrete catalog entries in upstream declaration order.
    pub fn all() -> &'static [Self] {
        &[
            Self::Ada24,
            Self::Ada32Pro,
            Self::Ada48Pro,
            Self::Ada80Pro,
            Self::Ampere16,
            Self::Ampere24,
            Self::Ampere48,
            Self::Ampere80,
            Self::Hopper141,
            Self::Blackwell96,
            Self::Blackwell180,
        ]
    }
}
impl fmt::Display for GpuGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for GpuGroup {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "any" => Ok(Self::Any),
            "ADA_24" => Ok(Self::Ada24),
            "ADA_32_PRO" => Ok(Self::Ada32Pro),
            "ADA_48_PRO" => Ok(Self::Ada48Pro),
            "ADA_80_PRO" => Ok(Self::Ada80Pro),
            "AMPERE_16" => Ok(Self::Ampere16),
            "AMPERE_24" => Ok(Self::Ampere24),
            "AMPERE_48" => Ok(Self::Ampere48),
            "AMPERE_80" => Ok(Self::Ampere80),
            "HOPPER_141" => Ok(Self::Hopper141),
            "BLACKWELL_96" => Ok(Self::Blackwell96),
            "BLACKWELL_180" => Ok(Self::Blackwell180),
            _ => Err(Error::InvalidInput(format!("unknown GpuGroup {value}"))),
        }
    }
}
/// Upstream GpuType catalog values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum GpuType {
    /// any.
    #[serde(rename = "any")]
    Any,
    /// NVIDIA GeForce RTX 4090.
    #[serde(rename = "NVIDIA GeForce RTX 4090")]
    NvidiaGeforceRtx4090,
    /// NVIDIA GeForce RTX 5090.
    #[serde(rename = "NVIDIA GeForce RTX 5090")]
    NvidiaGeforceRtx5090,
    /// NVIDIA RTX 6000 Ada Generation.
    #[serde(rename = "NVIDIA RTX 6000 Ada Generation")]
    NvidiaRtx6000AdaGeneration,
    /// NVIDIA RTX PRO 6000 Blackwell Server Edition.
    #[serde(rename = "NVIDIA RTX PRO 6000 Blackwell Server Edition")]
    NvidiaRtxPro6000BlackwellServerEdition,
    /// NVIDIA RTX PRO 6000 Blackwell Workstation Edition.
    #[serde(rename = "NVIDIA RTX PRO 6000 Blackwell Workstation Edition")]
    NvidiaRtxPro6000BlackwellWorkstationEdition,
    /// NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition.
    #[serde(rename = "NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition")]
    NvidiaRtxPro6000BlackwellMaxQWorkstationEdition,
    /// NVIDIA H100 80GB HBM3.
    #[serde(rename = "NVIDIA H100 80GB HBM3")]
    NvidiaH10080gbHbm3,
    /// NVIDIA H100 PCIe.
    #[serde(rename = "NVIDIA H100 PCIe")]
    NvidiaH100Pcie,
    /// NVIDIA H100 NVL.
    #[serde(rename = "NVIDIA H100 NVL")]
    NvidiaH100Nvl,
    /// NVIDIA RTX A4000.
    #[serde(rename = "NVIDIA RTX A4000")]
    NvidiaRtxA4000,
    /// NVIDIA RTX A4500.
    #[serde(rename = "NVIDIA RTX A4500")]
    NvidiaRtxA4500,
    /// NVIDIA RTX 4000 Ada Generation.
    #[serde(rename = "NVIDIA RTX 4000 Ada Generation")]
    NvidiaRtx4000AdaGeneration,
    /// NVIDIA RTX 2000 Ada Generation.
    #[serde(rename = "NVIDIA RTX 2000 Ada Generation")]
    NvidiaRtx2000AdaGeneration,
    /// NVIDIA RTX A5000.
    #[serde(rename = "NVIDIA RTX A5000")]
    NvidiaRtxA5000,
    /// NVIDIA L4.
    #[serde(rename = "NVIDIA L4")]
    NvidiaL4,
    /// NVIDIA GeForce RTX 3090.
    #[serde(rename = "NVIDIA GeForce RTX 3090")]
    NvidiaGeforceRtx3090,
    /// NVIDIA A40.
    #[serde(rename = "NVIDIA A40")]
    NvidiaA40,
    /// NVIDIA RTX A6000.
    #[serde(rename = "NVIDIA RTX A6000")]
    NvidiaRtxA6000,
    /// NVIDIA A100 80GB PCIe.
    #[serde(rename = "NVIDIA A100 80GB PCIe")]
    NvidiaA10080gbPcie,
    /// NVIDIA A100-SXM4-80GB.
    #[serde(rename = "NVIDIA A100-SXM4-80GB")]
    NvidiaA100Sxm480gb,
    /// NVIDIA H200.
    #[serde(rename = "NVIDIA H200")]
    NvidiaH200,
    /// NVIDIA B200.
    #[serde(rename = "NVIDIA B200")]
    NvidiaB200,
}
impl GpuType {
    /// Returns the platform's exact identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::NvidiaGeforceRtx4090 => "NVIDIA GeForce RTX 4090",
            Self::NvidiaGeforceRtx5090 => "NVIDIA GeForce RTX 5090",
            Self::NvidiaRtx6000AdaGeneration => "NVIDIA RTX 6000 Ada Generation",
            Self::NvidiaRtxPro6000BlackwellServerEdition => {
                "NVIDIA RTX PRO 6000 Blackwell Server Edition"
            }
            Self::NvidiaRtxPro6000BlackwellWorkstationEdition => {
                "NVIDIA RTX PRO 6000 Blackwell Workstation Edition"
            }
            Self::NvidiaRtxPro6000BlackwellMaxQWorkstationEdition => {
                "NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition"
            }
            Self::NvidiaH10080gbHbm3 => "NVIDIA H100 80GB HBM3",
            Self::NvidiaH100Pcie => "NVIDIA H100 PCIe",
            Self::NvidiaH100Nvl => "NVIDIA H100 NVL",
            Self::NvidiaRtxA4000 => "NVIDIA RTX A4000",
            Self::NvidiaRtxA4500 => "NVIDIA RTX A4500",
            Self::NvidiaRtx4000AdaGeneration => "NVIDIA RTX 4000 Ada Generation",
            Self::NvidiaRtx2000AdaGeneration => "NVIDIA RTX 2000 Ada Generation",
            Self::NvidiaRtxA5000 => "NVIDIA RTX A5000",
            Self::NvidiaL4 => "NVIDIA L4",
            Self::NvidiaGeforceRtx3090 => "NVIDIA GeForce RTX 3090",
            Self::NvidiaA40 => "NVIDIA A40",
            Self::NvidiaRtxA6000 => "NVIDIA RTX A6000",
            Self::NvidiaA10080gbPcie => "NVIDIA A100 80GB PCIe",
            Self::NvidiaA100Sxm480gb => "NVIDIA A100-SXM4-80GB",
            Self::NvidiaH200 => "NVIDIA H200",
            Self::NvidiaB200 => "NVIDIA B200",
        }
    }
    /// Returns concrete catalog entries in upstream declaration order.
    pub fn all() -> &'static [Self] {
        &[
            Self::NvidiaGeforceRtx4090,
            Self::NvidiaGeforceRtx5090,
            Self::NvidiaRtx6000AdaGeneration,
            Self::NvidiaRtxPro6000BlackwellServerEdition,
            Self::NvidiaRtxPro6000BlackwellWorkstationEdition,
            Self::NvidiaRtxPro6000BlackwellMaxQWorkstationEdition,
            Self::NvidiaH10080gbHbm3,
            Self::NvidiaH100Pcie,
            Self::NvidiaH100Nvl,
            Self::NvidiaRtxA4000,
            Self::NvidiaRtxA4500,
            Self::NvidiaRtx4000AdaGeneration,
            Self::NvidiaRtx2000AdaGeneration,
            Self::NvidiaRtxA5000,
            Self::NvidiaL4,
            Self::NvidiaGeforceRtx3090,
            Self::NvidiaA40,
            Self::NvidiaRtxA6000,
            Self::NvidiaA10080gbPcie,
            Self::NvidiaA100Sxm480gb,
            Self::NvidiaH200,
            Self::NvidiaB200,
        ]
    }
}
impl fmt::Display for GpuType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for GpuType {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "any" => Ok(Self::Any),
            "NVIDIA GeForce RTX 4090" => Ok(Self::NvidiaGeforceRtx4090),
            "NVIDIA GeForce RTX 5090" => Ok(Self::NvidiaGeforceRtx5090),
            "NVIDIA RTX 6000 Ada Generation" => Ok(Self::NvidiaRtx6000AdaGeneration),
            "NVIDIA RTX PRO 6000 Blackwell Server Edition" => {
                Ok(Self::NvidiaRtxPro6000BlackwellServerEdition)
            }
            "NVIDIA RTX PRO 6000 Blackwell Workstation Edition" => {
                Ok(Self::NvidiaRtxPro6000BlackwellWorkstationEdition)
            }
            "NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition" => {
                Ok(Self::NvidiaRtxPro6000BlackwellMaxQWorkstationEdition)
            }
            "NVIDIA H100 80GB HBM3" => Ok(Self::NvidiaH10080gbHbm3),
            "NVIDIA H100 PCIe" => Ok(Self::NvidiaH100Pcie),
            "NVIDIA H100 NVL" => Ok(Self::NvidiaH100Nvl),
            "NVIDIA RTX A4000" => Ok(Self::NvidiaRtxA4000),
            "NVIDIA RTX A4500" => Ok(Self::NvidiaRtxA4500),
            "NVIDIA RTX 4000 Ada Generation" => Ok(Self::NvidiaRtx4000AdaGeneration),
            "NVIDIA RTX 2000 Ada Generation" => Ok(Self::NvidiaRtx2000AdaGeneration),
            "NVIDIA RTX A5000" => Ok(Self::NvidiaRtxA5000),
            "NVIDIA L4" => Ok(Self::NvidiaL4),
            "NVIDIA GeForce RTX 3090" => Ok(Self::NvidiaGeforceRtx3090),
            "NVIDIA A40" => Ok(Self::NvidiaA40),
            "NVIDIA RTX A6000" => Ok(Self::NvidiaRtxA6000),
            "NVIDIA A100 80GB PCIe" => Ok(Self::NvidiaA10080gbPcie),
            "NVIDIA A100-SXM4-80GB" => Ok(Self::NvidiaA100Sxm480gb),
            "NVIDIA H200" => Ok(Self::NvidiaH200),
            "NVIDIA B200" => Ok(Self::NvidiaB200),
            _ => Err(Error::InvalidInput(format!("unknown GpuType {value}"))),
        }
    }
}
/// Upstream CpuInstanceType catalog values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CpuInstanceType {
    /// cpu3g-1-4.
    #[serde(rename = "cpu3g-1-4")]
    Cpu3g14,
    /// cpu3g-2-8.
    #[serde(rename = "cpu3g-2-8")]
    Cpu3g28,
    /// cpu3g-4-16.
    #[serde(rename = "cpu3g-4-16")]
    Cpu3g416,
    /// cpu3g-8-32.
    #[serde(rename = "cpu3g-8-32")]
    Cpu3g832,
    /// cpu3c-1-2.
    #[serde(rename = "cpu3c-1-2")]
    Cpu3c12,
    /// cpu3c-2-4.
    #[serde(rename = "cpu3c-2-4")]
    Cpu3c24,
    /// cpu3c-4-8.
    #[serde(rename = "cpu3c-4-8")]
    Cpu3c48,
    /// cpu3c-8-16.
    #[serde(rename = "cpu3c-8-16")]
    Cpu3c816,
    /// cpu5c-1-2.
    #[serde(rename = "cpu5c-1-2")]
    Cpu5c12,
    /// cpu5c-2-4.
    #[serde(rename = "cpu5c-2-4")]
    Cpu5c24,
    /// cpu5c-4-8.
    #[serde(rename = "cpu5c-4-8")]
    Cpu5c48,
    /// cpu5c-8-16.
    #[serde(rename = "cpu5c-8-16")]
    Cpu5c816,
}
impl CpuInstanceType {
    /// Returns the platform's exact identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu3g14 => "cpu3g-1-4",
            Self::Cpu3g28 => "cpu3g-2-8",
            Self::Cpu3g416 => "cpu3g-4-16",
            Self::Cpu3g832 => "cpu3g-8-32",
            Self::Cpu3c12 => "cpu3c-1-2",
            Self::Cpu3c24 => "cpu3c-2-4",
            Self::Cpu3c48 => "cpu3c-4-8",
            Self::Cpu3c816 => "cpu3c-8-16",
            Self::Cpu5c12 => "cpu5c-1-2",
            Self::Cpu5c24 => "cpu5c-2-4",
            Self::Cpu5c48 => "cpu5c-4-8",
            Self::Cpu5c816 => "cpu5c-8-16",
        }
    }
    /// Returns concrete catalog entries in upstream declaration order.
    pub fn all() -> &'static [Self] {
        &[
            Self::Cpu3g14,
            Self::Cpu3g28,
            Self::Cpu3g416,
            Self::Cpu3g832,
            Self::Cpu3c12,
            Self::Cpu3c24,
            Self::Cpu3c48,
            Self::Cpu3c816,
            Self::Cpu5c12,
            Self::Cpu5c24,
            Self::Cpu5c48,
            Self::Cpu5c816,
        ]
    }
}
impl fmt::Display for CpuInstanceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for CpuInstanceType {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "cpu3g-1-4" => Ok(Self::Cpu3g14),
            "cpu3g-2-8" => Ok(Self::Cpu3g28),
            "cpu3g-4-16" => Ok(Self::Cpu3g416),
            "cpu3g-8-32" => Ok(Self::Cpu3g832),
            "cpu3c-1-2" => Ok(Self::Cpu3c12),
            "cpu3c-2-4" => Ok(Self::Cpu3c24),
            "cpu3c-4-8" => Ok(Self::Cpu3c48),
            "cpu3c-8-16" => Ok(Self::Cpu3c816),
            "cpu5c-1-2" => Ok(Self::Cpu5c12),
            "cpu5c-2-4" => Ok(Self::Cpu5c24),
            "cpu5c-4-8" => Ok(Self::Cpu5c48),
            "cpu5c-8-16" => Ok(Self::Cpu5c816),
            _ => Err(Error::InvalidInput(format!(
                "unknown CpuInstanceType {value}"
            ))),
        }
    }
}
/// Upstream DataCenter catalog values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DataCenter {
    /// US-CA-2.
    #[serde(rename = "US-CA-2")]
    UsCa2,
    /// US-IL-1.
    #[serde(rename = "US-IL-1")]
    UsIl1,
    /// US-KS-2.
    #[serde(rename = "US-KS-2")]
    UsKs2,
    /// US-MO-1.
    #[serde(rename = "US-MO-1")]
    UsMo1,
    /// US-MO-2.
    #[serde(rename = "US-MO-2")]
    UsMo2,
    /// US-NC-2.
    #[serde(rename = "US-NC-2")]
    UsNc2,
    /// US-NE-1.
    #[serde(rename = "US-NE-1")]
    UsNe1,
    /// US-WA-1.
    #[serde(rename = "US-WA-1")]
    UsWa1,
    /// EU-CZ-1.
    #[serde(rename = "EU-CZ-1")]
    EuCz1,
    /// EU-RO-1.
    #[serde(rename = "EU-RO-1")]
    EuRo1,
    /// EUR-NO-1.
    #[serde(rename = "EUR-NO-1")]
    EurNo1,
}
impl DataCenter {
    /// Returns the platform's exact identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsCa2 => "US-CA-2",
            Self::UsIl1 => "US-IL-1",
            Self::UsKs2 => "US-KS-2",
            Self::UsMo1 => "US-MO-1",
            Self::UsMo2 => "US-MO-2",
            Self::UsNc2 => "US-NC-2",
            Self::UsNe1 => "US-NE-1",
            Self::UsWa1 => "US-WA-1",
            Self::EuCz1 => "EU-CZ-1",
            Self::EuRo1 => "EU-RO-1",
            Self::EurNo1 => "EUR-NO-1",
        }
    }
    /// Returns concrete catalog entries in upstream declaration order.
    pub fn all() -> &'static [Self] {
        &[
            Self::UsCa2,
            Self::UsIl1,
            Self::UsKs2,
            Self::UsMo1,
            Self::UsMo2,
            Self::UsNc2,
            Self::UsNe1,
            Self::UsWa1,
            Self::EuCz1,
            Self::EuRo1,
            Self::EurNo1,
        ]
    }
}
impl fmt::Display for DataCenter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for DataCenter {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let normalized = value.trim().to_uppercase().replace('_', "-");
        match normalized.as_str() {
            "US-CA-2" => Ok(Self::UsCa2),
            "US-IL-1" => Ok(Self::UsIl1),
            "US-KS-2" => Ok(Self::UsKs2),
            "US-MO-1" => Ok(Self::UsMo1),
            "US-MO-2" => Ok(Self::UsMo2),
            "US-NC-2" => Ok(Self::UsNc2),
            "US-NE-1" => Ok(Self::UsNe1),
            "US-WA-1" => Ok(Self::UsWa1),
            "EU-CZ-1" => Ok(Self::EuCz1),
            "EU-RO-1" => Ok(Self::EuRo1),
            "EUR-NO-1" => Ok(Self::EurNo1),
            _ => Err(Error::InvalidInput(format!("unknown DataCenter {value}"))),
        }
    }
}
impl GpuGroup {
    /// Returns the statically pinned members of this GPU pool.
    pub fn device_names(self) -> &'static [GpuType] {
        match self {
            Self::Ada24 => &[GpuType::NvidiaGeforceRtx4090],
            Self::Ada32Pro => &[GpuType::NvidiaGeforceRtx5090],
            Self::Ada48Pro => &[GpuType::NvidiaRtx6000AdaGeneration],
            Self::Ada80Pro => &[
                GpuType::NvidiaH10080gbHbm3,
                GpuType::NvidiaH100Pcie,
                GpuType::NvidiaH100Nvl,
            ],
            Self::Ampere16 => &[
                GpuType::NvidiaRtxA4000,
                GpuType::NvidiaRtxA4500,
                GpuType::NvidiaRtx4000AdaGeneration,
                GpuType::NvidiaRtx2000AdaGeneration,
            ],
            Self::Ampere24 => &[
                GpuType::NvidiaRtxA5000,
                GpuType::NvidiaL4,
                GpuType::NvidiaGeforceRtx3090,
            ],
            Self::Ampere48 => &[GpuType::NvidiaA40, GpuType::NvidiaRtxA6000],
            Self::Ampere80 => &[GpuType::NvidiaA10080gbPcie, GpuType::NvidiaA100Sxm480gb],
            Self::Hopper141 => &[GpuType::NvidiaH200],
            Self::Blackwell96 => &[
                GpuType::NvidiaRtxPro6000BlackwellServerEdition,
                GpuType::NvidiaRtxPro6000BlackwellWorkstationEdition,
                GpuType::NvidiaRtxPro6000BlackwellMaxQWorkstationEdition,
            ],
            Self::Blackwell180 => &[GpuType::NvidiaB200],
            Self::Any => &[],
        }
    }
}
/// Returns the static pool containing the exact GPU device.
pub fn pool_for_gpu_type(device: GpuType) -> Option<GpuGroup> {
    GpuGroup::all()
        .iter()
        .copied()
        .find(|pool| pool.device_names().contains(&device))
}
