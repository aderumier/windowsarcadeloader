//! Native Sinden border selection. Detection reads sysfs, so input-device permissions do
//! not prevent activation. The Sinden driver still supplies the gun's coordinates.
use crate::config::Profile;
use anyhow::{Result, bail};
use wal_protocol::border::Border;

pub fn detected() -> bool {
    ["/sys/class/input", "/sys/bus/usb/devices"]
        .iter()
        .any(|root| {
            std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| {
                    ["device/name", "product", "manufacturer"]
                        .iter()
                        .any(|name| {
                            std::fs::read_to_string(entry.path().join(name))
                                .is_ok_and(|v| v.to_ascii_lowercase().contains("sinden"))
                        })
                })
        })
}

pub fn select(profile: &Profile) -> Result<Option<Border>> {
    let value = |key: &str, default: &str| {
        profile
            .env
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.into())
    };
    let enabled = match value("WAL_SINDEN_BORDER", "auto").trim() {
        "auto" => {
            profile.input.guns_enabled
                && !profile.input.mouse_wrap
                && (detected() || value("WAL_SINDEN_DETECTED", "0") == "1")
        }
        "1" | "on" | "true" => true,
        "0" | "off" | "false" => false,
        _ => bail!("WAL_SINDEN_BORDER: expected auto, 1, or 0"),
    };
    if !enabled {
        return Ok(None);
    }
    Ok(Some(
        Border::parse(
            &value("WAL_SINDEN_BORDER_SIZE", "medium"),
            &value("WAL_SINDEN_BORDER_COLOR", "white"),
        )
        .map_err(anyhow::Error::msg)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_manual_and_batocera_detection() {
        let mut profile =
            Profile::without_game(std::path::Path::new("/tmp/wal-sinden-no-config")).unwrap();
        profile.env.insert("WAL_SINDEN_DETECTED".into(), "1".into());
        assert!(select(&profile).unwrap().is_none()); // non-gun game
        profile.input.guns_enabled = true;
        assert!(select(&profile).unwrap().is_some());
        profile.env.insert("WAL_SINDEN_BORDER".into(), "0".into());
        assert!(select(&profile).unwrap().is_none());
        profile.input.guns_enabled = false;
        profile.env.insert("WAL_SINDEN_BORDER".into(), "1".into());
        assert!(select(&profile).unwrap().is_some());
        profile
            .env
            .insert("WAL_SINDEN_BORDER_SIZE".into(), "0".into());
        assert!(select(&profile).is_err());
    }
}
