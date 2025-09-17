use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(
    clap::ValueEnum, Default, Debug, PartialEq, PartialOrd, Clone, Copy, Deserialize, Serialize,
)]
pub enum HexdumpLevel {
    #[default]
    Disabled,
    DecryptedInput,
    RawInput,
    DecryptedOutput,
    RawOutput,
    All,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
}

impl std::str::FromStr for UsbId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err("Expected format VID:PID".to_string());
        }
        let vid = u16::from_str_radix(parts[0], 16).map_err(|e| e.to_string())?;
        let pid = u16::from_str_radix(parts[1], 16).map_err(|e| e.to_string())?;
        Ok(UsbId { vid, pid })
    }
}

impl fmt::Display for UsbId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}:{:x}", self.vid, self.pid)
    }
}

impl<'de> Deserialize<'de> for UsbId {
    fn deserialize<D>(deserializer: D) -> Result<UsbId, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UsbIdVisitor;

        impl<'de> Visitor<'de> for UsbIdVisitor {
            type Value = UsbId;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string in the format VID:PID")
            }

            fn visit_str<E>(self, value: &str) -> Result<UsbId, E>
            where
                E: de::Error,
            {
                UsbId::from_str(value).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(UsbIdVisitor)
    }
}
