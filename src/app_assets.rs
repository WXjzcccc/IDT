use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use gpui_component_assets::Assets as ComponentAssets;

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "icons/tags.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/tags.svg"
            )))),
            "icons/picture-in-picture.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/picture-in-picture.svg"
            )))),
            "icons/pin.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/pin.svg"
            )))),
            "icons/pin-off.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/pin-off.svg"
            )))),
            "icons/lock.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/lock.svg"
            )))),
            "icons/lock-open.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/lock-open.svg"
            )))),
            "icons/monitor-stop.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/monitor-stop.svg"
            )))),
            _ => ComponentAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut assets = ComponentAssets.list(path)?;
        for local in [
            "icons/tags.svg",
            "icons/picture-in-picture.svg",
            "icons/pin.svg",
            "icons/pin-off.svg",
            "icons/lock.svg",
            "icons/lock-open.svg",
            "icons/monitor-stop.svg",
        ] {
            if local.starts_with(path) && !assets.iter().any(|asset| asset.as_ref() == local) {
                assets.push(local.into());
            }
        }
        Ok(assets)
    }
}
