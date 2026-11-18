//! The GitHub release check behind the "new version available" notice (D29,
//! D31).
//!
//! CodingPad has no auto-updater. Once per launch the app asks GitHub for the
//! newest published release; when it is newer than this build the desktop shows
//! a notice linking to that release's page. This is the only network request the
//! app makes on its own.
//!
//! Every failure is silent and leaves the app unchanged: no release published
//! yet (GitHub answers 404), no connection, a rate limit, or an unexpected
//! payload all simply mean "no notice". Only plain numbered releases count, and
//! only release pages under this repository are ever linked.

use serde::Serialize;
use std::time::Duration;

/// The newest published release, excluding drafts and pre-releases.
pub const RELEASES_URL: &str = "https://api.github.com/repos/joeizang/codingpad/releases/latest";

/// Release pages must live here before the app links to them.
pub const RELEASE_PAGE_PREFIX: &str = "https://github.com/joeizang/codingpad/releases/";

/// The most GitHub may send for one release; a larger answer is ignored.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// A release newer than the running build, prepared for the notice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotice {
    /// The release version without its leading `v`, for example `0.2.0`.
    pub version: String,
    /// The GitHub release page, always under [`RELEASE_PAGE_PREFIX`].
    pub url: String,
}

/// A numeric `major.minor.patch` version. Missing parts count as zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl Version {
    /// Parse `0.2`, `v0.2.0` and similar tags. Anything carrying a pre-release
    /// or build suffix, extra parts, or words is not a plain release and is
    /// ignored, so a `0.2.0-beta` tag never offers a beta as an update.
    fn parse(text: &str) -> Option<Self> {
        let mut parts = trim_tag(text.trim()).split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self(major, minor, patch))
    }
}

/// Drop a single leading `v` from a release tag.
fn trim_tag(tag: &str) -> &str {
    tag.strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag)
}

/// Read a GitHub release payload. `None` covers a current or older build, a
/// payload that is not a release notice, and anything malformed.
fn notice_from_release(current_version: &str, payload: &str) -> Option<UpdateNotice> {
    let current = Version::parse(current_version)?;
    let release: serde_json::Value = serde_json::from_str(payload).ok()?;
    if release.get("draft").and_then(serde_json::Value::as_bool) == Some(true)
        || release
            .get("prerelease")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
    {
        return None;
    }
    let tag = release.get("tag_name")?.as_str()?.trim();
    if Version::parse(tag)? <= current {
        return None;
    }
    let url = release.get("html_url")?.as_str()?;
    if !url.starts_with(RELEASE_PAGE_PREFIX) || url.chars().any(char::is_whitespace) {
        return None;
    }
    Some(UpdateNotice {
        version: trim_tag(tag).to_owned(),
        url: url.to_owned(),
    })
}

/// Ask GitHub for the newest release. Every failure returns `None` on purpose;
/// the caller never shows an error for a check that did not work out.
pub async fn check(current_version: &str) -> Option<UpdateNotice> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        // A notice is never worth waiting longer for.
        .timeout(Duration::from_secs(15))
        .build()
        .ok()?;
    let mut response = client
        .get(RELEASES_URL)
        // GitHub rejects requests without a user agent.
        .header(
            reqwest::header::USER_AGENT,
            format!("CodingPad/{current_version}"),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    // A 404 means no release has been published yet, which is not an error.
    if !response.status().is_success() {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    notice_from_release(current_version, &String::from_utf8(body).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, url: &str) -> String {
        serde_json::json!({
            "tag_name": tag,
            "html_url": url,
            "draft": false,
            "prerelease": false,
        })
        .to_string()
    }

    const PAGE: &str = "https://github.com/joeizang/codingpad/releases/tag/v0.2.0";

    #[test]
    fn an_older_build_is_told_about_the_new_release() {
        let notice = notice_from_release("0.1.0", &release("v0.2.0", PAGE)).unwrap();
        assert_eq!(notice.version, "0.2.0");
        assert_eq!(notice.url, PAGE);
    }

    #[test]
    fn a_current_build_sees_no_notice() {
        assert_eq!(notice_from_release("0.2.0", &release("v0.2.0", PAGE)), None);
        assert_eq!(notice_from_release("0.2.0", &release("0.2", PAGE)), None);
    }

    #[test]
    fn a_newer_build_sees_no_notice() {
        assert_eq!(notice_from_release("0.3.0", &release("v0.2.0", PAGE)), None);
        assert_eq!(
            notice_from_release("1.0.0", &release("v0.99.0", PAGE)),
            None
        );
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        let notice = notice_from_release("0.9.0", &release("v0.10.0", PAGE)).unwrap();
        assert_eq!(notice.version, "0.10.0");
        assert_eq!(
            notice_from_release("0.10.0", &release("v0.9.0", PAGE)),
            None
        );
    }

    #[test]
    fn tags_may_carry_a_v_and_omit_the_patch() {
        let notice = notice_from_release("0.1.0", &release("0.2", PAGE)).unwrap();
        assert_eq!(notice.version, "0.2");
        let notice = notice_from_release("0.1.0", &release("V0.2.0", PAGE)).unwrap();
        assert_eq!(notice.version, "0.2.0");
    }

    #[test]
    fn pre_releases_and_unusual_tags_are_ignored() {
        for tag in [
            "v0.2.0-beta.1",
            "v0.2.0+build.5",
            "nightly",
            "v0.2.0.1",
            "v0..1",
        ] {
            assert_eq!(notice_from_release("0.1.0", &release(tag, PAGE)), None);
        }
    }

    #[test]
    fn draft_and_prerelease_responses_are_ignored() {
        for payload in [
            serde_json::json!({
                "tag_name": "v0.2.0",
                "html_url": PAGE,
                "draft": true,
            }),
            serde_json::json!({
                "tag_name": "v0.2.0",
                "html_url": PAGE,
                "prerelease": true,
            }),
        ] {
            assert_eq!(notice_from_release("0.1.0", &payload.to_string()), None);
        }
    }

    #[test]
    fn only_release_pages_in_this_repository_are_linked() {
        for url in [
            "https://example.com/releases/tag/v0.2.0",
            "http://github.com/joeizang/codingpad/releases/tag/v0.2.0",
            "https://github.com/someone/codingpad/releases/tag/v0.2.0",
            "https://github.com/joeizang/codingpad/releases/tag/v0.2.0\n",
        ] {
            assert_eq!(notice_from_release("0.1.0", &release("v0.2.0", url)), None);
        }
    }

    #[test]
    fn malformed_payloads_are_ignored() {
        for payload in [
            "",
            "not json",
            "{}",
            r#"{"tag_name": 2, "html_url": "https://github.com/joeizang/codingpad/releases/tag/v0.2.0"}"#,
            r#"{"tag_name": "v0.2.0"}"#,
        ] {
            assert_eq!(notice_from_release("0.1.0", payload), None);
        }
    }

    #[test]
    fn the_check_targets_this_repository() {
        assert_eq!(
            RELEASES_URL,
            "https://api.github.com/repos/joeizang/codingpad/releases/latest"
        );
        assert!(PAGE.starts_with(RELEASE_PAGE_PREFIX));
    }
}
