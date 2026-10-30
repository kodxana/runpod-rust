use std::{collections::BTreeMap, fmt, net::Ipv4Addr};

/// Snapshot of `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` and `NO_PROXY`, with lowercase precedence.
///
/// System proxy registries and PAC files are deliberately not consulted. The
/// downloader disables implicit reqwest proxies and uses only this snapshot.
#[derive(Clone, Default)]
pub struct ProxySettings {
    proxies: BTreeMap<String, String>,
}

impl fmt::Debug for ProxySettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxySettings")
            .field("schemes", &self.proxies.keys())
            .finish()
    }
}

impl ProxySettings {
    /// Capture the process environment without changing it.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_values(&super::environment_values())
    }

    /// Parse a supplied environment using urllib's two-pass precedence rules.
    #[must_use]
    pub fn from_values(environment: &BTreeMap<String, String>) -> Self {
        let mut proxies = BTreeMap::new();
        for (name, value) in environment {
            if let Some(scheme) = name.to_ascii_lowercase().strip_suffix("_proxy")
                && !value.is_empty()
            {
                proxies.insert(scheme.to_owned(), value.clone());
            }
        }
        if environment.contains_key("REQUEST_METHOD") {
            proxies.remove("http");
        }
        for (name, value) in environment {
            if let Some(scheme) = name.strip_suffix("_proxy") {
                let scheme = scheme.to_ascii_lowercase();
                if value.is_empty() {
                    proxies.remove(&scheme);
                } else {
                    proxies.insert(scheme, value.clone());
                }
            }
        }
        Self { proxies }
    }

    pub(super) fn selected(
        &self,
        url: &reqwest::Url,
        host: &str,
        port: Option<u16>,
    ) -> Option<&str> {
        if self
            .proxies
            .get("no")
            .is_some_and(|value| bypass(host, port, value))
        {
            return None;
        }
        self.proxies
            .get(&format!("{}://{host}", url.scheme()))
            .or_else(|| self.proxies.get(url.scheme()))
            .or_else(|| self.proxies.get(&format!("all://{host}")))
            .or_else(|| self.proxies.get("all"))
            .map(String::as_str)
    }
}

fn bypass(host: &str, port: Option<u16>, exclusions: &str) -> bool {
    if exclusions == "*" {
        return true;
    }
    let with_port = port.filter(|p| *p != 0).map(|p| format!("{host}:{p}"));
    for entry in exclusions
        .replace(' ', "")
        .split(',')
        .filter(|s| !s.is_empty())
    {
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            if entry == host {
                return true;
            }
            if let Some((base, bits)) = entry.split_once('/')
                && let (Ok(base), Ok(bits)) = (base.parse::<Ipv4Addr>(), bits.parse::<u32>())
                && (1..=32).contains(&bits)
                && u32::from(ip) >> (32 - bits) == u32::from(base) >> (32 - bits)
            {
                return true;
            }
        } else {
            let entry = entry.trim_start_matches('.');
            if host == entry
                || host.ends_with(&format!(".{entry}"))
                || with_port
                    .as_deref()
                    .is_some_and(|hp| hp == entry || hp.ends_with(&format!(".{entry}")))
            {
                return true;
            }
        }
        // urllib's environment fallback is case insensitive and also applies
        // its domain-style match to IP literals.
        let entry = entry.trim_start_matches('.').to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        if host == entry || host.ends_with(&format!(".{entry}")) {
            return true;
        }
    }
    false
}
