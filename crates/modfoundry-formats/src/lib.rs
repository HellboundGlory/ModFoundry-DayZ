//! The DayZ and Enfusion file families ModFoundry knows, as `formats.list` reports them.

use serde::{Deserialize, Serialize};

/// All DayZ/Enfusion file families ModFoundry understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    /// DayZ/Enfusion PBO container (property-text header variant; third-party
    /// "obfuscated" files are name-mangled but standard).
    Pbo,
    /// Editable P3D (MLOD, signature `MLOD`, version 0x101).
    P3dMlod,
    /// Binarized P3D (ODOL v7/v4x for DayZ).
    P3dOdol,
    /// RV/Enfusion texture container.
    Paa,
    /// Legacy texture container (pre-PAA; used by Object Builder tooling).
    Pac,
    /// Binarized config (`config.bin`, written by CfgConvert).
    ConfigBin,
    /// Text config (`config.cpp`, RVMat, etc. — C-like superset).
    ConfigText,
    /// World file (`.wrp`).
    Wrp,
    /// Material file (`.rvmat`).
    RvMat,
    /// DayZ signature (`<file>.bisign`).
    Bisign,
    /// Public key (`.bikey`) / private key (`.biprivatekey`).
    Bikey,
    /// Enfusion game project (`dayz.gproj`).
    Gproj,
    /// Enforce Script source (`.c` inside script modules — see modfoundry-enforce).
    EnforceScript,
    /// Central Economy / mission XML (types.xml, events.xml, cfgeconomycore.xml…).
    CeXml,
}

impl Format {
    /// Every family ModFoundry recognizes, in declaration order. This is what
    /// `mf formats` / the `formats.list` command report.
    pub fn all() -> &'static [Format] {
        &[
            Format::Pbo,
            Format::P3dMlod,
            Format::P3dOdol,
            Format::Paa,
            Format::Pac,
            Format::ConfigBin,
            Format::ConfigText,
            Format::Wrp,
            Format::RvMat,
            Format::Bisign,
            Format::Bikey,
            Format::Gproj,
            Format::EnforceScript,
            Format::CeXml,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_each_family_once() {
        let all = Format::all();
        for (i, f) in all.iter().enumerate() {
            assert!(!all[..i].contains(f), "{f:?} listed twice in Format::all()");
        }
    }
}
