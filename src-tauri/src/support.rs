//! Settings → About's support links: GitHub Sponsors and Ko-fi.
//!
//! - **`open_support_page(page)`** opens the constant address of `page` (`"github_sponsors"` or
//!   `"ko_fi"`) in the default browser with `xdg-open`. The frontend names a page, never an
//!   address: nothing it sends is opened.

use serde::Deserialize;

use crate::gallery;

/// The GitHub Sponsors profile; the same account as `github` in `.github/FUNDING.yml`.
pub const GITHUB_SPONSORS: &str = "https://github.com/sponsors/FabioSM46";

/// The Ko-fi page; the same account as `ko_fi` in `.github/FUNDING.yml`.
pub const KO_FI: &str = "https://ko-fi.com/fabiosm46";

/// A page `open_support_page` can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportPage {
    GithubSponsors,
    KoFi,
}

impl SupportPage {
    /// The address this page opens.
    pub fn url(self) -> &'static str {
        match self {
            SupportPage::GithubSponsors => GITHUB_SPONSORS,
            SupportPage::KoFi => KO_FI,
        }
    }
}

/// Opens `page` in the default browser.
#[tauri::command]
pub fn open_support_page(page: SupportPage) -> Result<(), String> {
    gallery::xdg_open(page.url())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_named_as_the_frontend_sends_them() {
        let page: SupportPage = serde_json::from_str("\"github_sponsors\"").unwrap();
        assert_eq!(page, SupportPage::GithubSponsors);
        let page: SupportPage = serde_json::from_str("\"ko_fi\"").unwrap();
        assert_eq!(page, SupportPage::KoFi);
    }

    #[test]
    fn an_address_is_not_a_page() {
        assert!(serde_json::from_str::<SupportPage>("\"https://example.com\"").is_err());
    }

    #[test]
    fn every_page_opens_an_https_address() {
        for page in [SupportPage::GithubSponsors, SupportPage::KoFi] {
            assert!(page.url().starts_with("https://"), "{page:?}");
        }
    }

    #[test]
    fn the_addresses_match_the_funding_file() {
        let funding = include_str!("../../.github/FUNDING.yml");
        assert!(funding.contains("github: FabioSM46"));
        assert!(GITHUB_SPONSORS.ends_with("/FabioSM46"));
        assert!(funding.contains("ko_fi: fabiosm46"));
        assert!(KO_FI.ends_with("/fabiosm46"));
    }
}
