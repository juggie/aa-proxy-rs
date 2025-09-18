use bluer::Address;
use serde::{
    de::{self, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use std::{fmt, str::FromStr};

#[derive(Debug, Clone)]
pub struct BluetoothAddressList(pub Option<Vec<Address>>);

impl BluetoothAddressList {
    fn to_string_internal(&self) -> String {
        match &self.0 {
            Some(addresses) => addresses
                .iter()
                .map(|addr| addr.to_string())
                .collect::<Vec<_>>()
                .join(","),
            None => "".to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for BluetoothAddressList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        if s.is_empty() {
            return Ok(BluetoothAddressList(None));
        }

        let addresses: Result<Vec<Address>, _> = s
            .split(',')
            .map(|addr_str| addr_str.trim().parse::<Address>())
            .collect();

        match addresses {
            Ok(addrs) => {
                let wildcard_present = addrs.iter().any(|addr| addr == &Address::any());

                if wildcard_present && addrs.len() > 1 {
                    return Err(de::Error::custom(
                        "'connect' - Wildcard address '00:00:00:00:00:00' cannot be combined with other addresses"
                    ));
                }
                Ok(BluetoothAddressList(Some(addrs)))
            }
            Err(e) => Err(de::Error::custom(format!(
                "'connect' - Failed to parse addresses: {}",
                e
            ))),
        }
    }
}

impl Serialize for BluetoothAddressList {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let addresses_str = self.to_string_internal();
        serializer.serialize_str(&addresses_str)
    }
}

impl fmt::Display for BluetoothAddressList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_internal())
    }
}

impl Default for BluetoothAddressList {
    fn default() -> Self {
        BluetoothAddressList(Some(vec![Address::any()]))
    }
}

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
