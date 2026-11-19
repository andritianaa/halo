//! Wi-Fi passwords: the networks this PC has saved and their keys, read from Windows'
//! own profile store. Opt-in (sensitive); nothing is stored or sent by Halo, the
//! profiles are read when the page asks for them.

use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};

pub const MANIFEST: Manifest = Manifest {
    id: "wifikeys",
    cdc: "M5+",
    name_key: "module.wifikeys",
    category: Category::Network,
    icon: "key-round",
    permissions: &[Permission::Elevation],
    opt_in: true,
    default_enabled: false,
};

/// One saved network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiKey {
    /// Profile name (usually the SSID).
    pub name: String,
    /// Network name (SSID) from the profile, when present.
    pub ssid: String,
    /// "WPA2PSK", "WPA3SAE", "open"…
    pub auth: String,
    /// The password; `None` for open/enterprise networks or when Windows kept it encrypted.
    pub key: Option<String>,
    /// Windows returned the key encrypted (Halo is not running as administrator).
    pub protected: bool,
}

impl WifiKey {
    /// Human label for the security type.
    pub fn security(&self) -> &'static str {
        match self.auth.to_ascii_lowercase().as_str() {
            "open" => "Open",
            "wpa3sae" | "wpa3" => "WPA3",
            "wpa2psk" | "wpa2" => "WPA2",
            "wpapsk" | "wpa" => "WPA",
            "shared" => "WEP",
            a if a.contains("wpa3") => "WPA3",
            a if a.contains("wpa2") => "WPA2",
            _ => "Enterprise",
        }
    }
}

/// Text of the first `<tag>…</tag>` (namespaces ignored).
fn tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(&xml[start..end])
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Parse a WLAN profile XML (as returned by `WlanGetProfile`).
pub fn parse_profile(name: &str, xml: &str) -> WifiKey {
    let ssid = tag(xml, "SSID").and_then(|s| tag(s, "name")).map(unescape).unwrap_or_else(|| name.to_string());
    let auth = tag(xml, "authentication").unwrap_or("").to_string();
    let protected = tag(xml, "protected").is_some_and(|p| p.trim() == "true");
    let key = if protected { None } else { tag(xml, "keyMaterial").map(unescape).filter(|k| !k.is_empty()) };
    WifiKey { name: name.to_string(), ssid, auth, key, protected }
}

/// All saved networks, sorted by name.
#[cfg(windows)]
pub fn saved() -> Vec<WifiKey> {
    let mut v: Vec<WifiKey> = halo_platform::sys::wifi_profiles().iter().map(|(n, x)| parse_profile(n, x)).collect();
    v.sort_by_key(|k| k.name.to_lowercase());
    v
}

#[cfg(not(windows))]
pub fn saved() -> Vec<WifiKey> {
    Vec::new()
}

/// Registry entry: no background work, only the opt-in switch.
#[derive(Default)]
pub struct WifiKeys;

impl Module for WifiKeys {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }
    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const WPA2: &str = r#"<?xml version="1.0"?>
<WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1">
  <name>Home &amp; Co</name>
  <SSIDConfig><SSID><hex>486F6D65</hex><name>Home &amp; Co</name></SSID></SSIDConfig>
  <MSM><security>
    <authEncryption><authentication>WPA2PSK</authentication><encryption>AES</encryption></authEncryption>
    <sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>s3cret&lt;!&gt;</keyMaterial></sharedKey>
  </security></MSM>
</WLANProfile>"#;

    #[test]
    fn plain_key_is_read_and_unescaped() {
        let k = parse_profile("Home & Co", WPA2);
        assert_eq!(k.ssid, "Home & Co");
        assert_eq!(k.key.as_deref(), Some("s3cret<!>"));
        assert!(!k.protected);
        assert_eq!(k.security(), "WPA2");
    }

    #[test]
    fn encrypted_or_open_networks_have_no_key() {
        let enc = WPA2.replace("<protected>false</protected>", "<protected>true</protected>").replace("s3cret&lt;!&gt;", "01000000D08C9DDF");
        let k = parse_profile("Home", &enc);
        assert!(k.protected && k.key.is_none());
        let open = r#"<WLANProfile><name>Cafe</name><MSM><security><authEncryption><authentication>open</authentication></authEncryption></security></MSM></WLANProfile>"#;
        let k = parse_profile("Cafe", open);
        assert_eq!((k.key.clone(), k.security(), k.ssid.as_str()), (None, "Open", "Cafe"));
    }
}
