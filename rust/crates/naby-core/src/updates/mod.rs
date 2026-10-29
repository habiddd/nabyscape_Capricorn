//! Ported from `Application/{OfficialUpdates,RuntimeUpdates,RuntimePlatform}.cs`.
//!
//! The official-release check (GitHub latest release, strict identity/asset validation, 24 h
//! throttle), checksum parsing, the redirect policy and the verified setup download, all behind
//! an injected `get` so every failure path runs offline. Launching the installer, Authenticode
//! verification and the version-resource check are host work (`naby-win`); `download_setup`
//! calls an injected `inspect` for them.

pub mod runtime;
mod sha256;
#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

pub use sha256::{hex_upper, sha256_hex, Sha256};

/// Name of the persisted throttle document (`update-state.json`).
pub const DOCUMENT_NAME: &str = "update-state";
pub const REPOSITORY: &str = "https://github.com/BillyNabil/nabyscape";
pub const ENDPOINT: &str = "https://api.github.com/repos/BillyNabil/nabyscape/releases/latest";
/// Asset holding `<sha256>  <setup name>` lines.
pub const CHECKSUMS_ASSET: &str = "SHA256SUMS";
pub const METADATA_LIMIT: usize = 1024 * 1024;
pub const CHECKSUM_LIMIT: usize = 65_536;
pub const MAX_SETUP_SIZE: u64 = 512 * 1024 * 1024;
/// Delay before the single retry of a 429 / 5xx response.
pub const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Minimum gap between automatic checks.
pub const CHECK_INTERVAL_HOURS: i64 = 24;
const SETUP_FILE: &str = "NabyScape-Setup.exe";
const REDIRECT_HOST: &str = "release-assets.githubusercontent.com";

/// A numeric `major.minor.patch` version (the stable-release subset of .NET `Version`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// The running build's version (`CARGO_PKG_VERSION`).
    pub fn installed() -> Self {
        env!("CARGO_PKG_VERSION")
            .parse()
            .unwrap_or(Self::new(0, 0, 0))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = UpdateError;
    /// Exactly three dot-separated decimal numbers; anything else (including pre-release
    /// suffixes and numbers above `i32::MAX`) is rejected.
    fn from_str(s: &str) -> Result<Self, UpdateError> {
        let bad = || UpdateError::Invalid(format!("Invalid version: {s}"));
        let mut parts = s.split('.');
        let mut next = || -> Result<u32, UpdateError> {
            let p = parts.next().ok_or_else(bad)?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            p.parse::<u32>()
                .ok()
                .filter(|&n| n <= i32::MAX as u32)
                .ok_or_else(bad)
        };
        let v = Self::new(next()?, next()?, next()?);
        if parts.next().is_some() {
            return Err(bad());
        }
        Ok(v)
    }
}

/// Orders two version strings; `None` when either is not `N.N.N`.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(a.parse::<Version>().ok()?.cmp(&b.parse::<Version>().ok()?))
}

/// The architecture a setup is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub rid: &'static str,
    pub pe_machine: u16,
}

impl Platform {
    pub const X64: Self = Self {
        rid: "win-x64",
        pe_machine: 0x8664,
    };
    pub const ARM64: Self = Self {
        rid: "win-arm64",
        pe_machine: 0xAA64,
    };

    /// The running binary's architecture; `None` when unsupported.
    pub fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::ARM64)
        } else {
            None
        }
    }

    /// C# `RuntimePlatform.SetupAsset`.
    pub fn setup_asset(&self) -> String {
        format!("NabyScape-{}-Setup.exe", self.rid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Metadata, checksum, redirect, size or file validation failed.
    Invalid(String),
    /// Offline, timeout or a non-success HTTP status.
    Network(String),
    /// Local file-system failure.
    Io(String),
    Cancelled,
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(m) | Self::Network(m) | Self::Io(m) => f.write_str(m),
            Self::Cancelled => {
                f.write_str("Update cancelled or timed out. Your current app is unchanged.")
            }
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

fn invalid(message: &str) -> UpdateError {
    UpdateError::Invalid(message.to_owned())
}

/// An official stable release newer than the running build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub notes: String,
    pub setup: String,
    pub size: u64,
    pub checksums: String,
}

/// Persisted throttle state (`update-state.json`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct UpdateState {
    pub last_attempt: DateTime<Utc>,
}

/// C# `OfficialUpdates.Check` gate: manual checks always run; automatic ones need the
/// preference and 24 h since the last attempt.
pub fn check_due(state: &UpdateState, manual: bool, enabled: bool, now: DateTime<Utc>) -> bool {
    manual || (enabled && now - state.last_attempt >= ChronoDuration::hours(CHECK_INTERVAL_HOURS))
}

/// The tag shape `^v\d+\.\d+\.\d+$` and its version.
fn tag_version(tag: &str) -> Option<Version> {
    tag.strip_prefix('v')?.parse().ok()
}

fn prop<'a>(value: &'a Value, name: &str) -> Result<&'a Value, UpdateError> {
    value
        .get(name)
        .ok_or_else(|| UpdateError::Invalid(format!("Release metadata is missing '{name}'.")))
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, UpdateError> {
    prop(value, name)?
        .as_str()
        .ok_or_else(|| UpdateError::Invalid(format!("Release metadata '{name}' is not text.")))
}

fn boolean(value: &Value, name: &str) -> Result<bool, UpdateError> {
    prop(value, name)?
        .as_bool()
        .ok_or_else(|| UpdateError::Invalid(format!("Release metadata '{name}' is not a boolean.")))
}

fn absolute(url: &str) -> Result<String, UpdateError> {
    Url::parse(url)
        .map(|u| u.to_string())
        .map_err(|_| invalid("Invalid release URL."))
}

impl Release {
    /// C# `OfficialUpdates.Parse`: validates a GitHub "latest release" document and returns the
    /// release if it is a newer stable one for `platform`, `Ok(None)` if not newer.
    pub fn parse(
        json: &str,
        current: Version,
        platform: &Platform,
    ) -> Result<Option<Self>, UpdateError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| UpdateError::Invalid(e.to_string()))?;
        let tag = string(&root, "tag_name")?;
        let version =
            tag_version(tag).ok_or_else(|| invalid("Expected a stable official release."))?;
        if boolean(&root, "draft")? || boolean(&root, "prerelease")? {
            return Err(invalid("Expected a stable official release."));
        }
        let notes = absolute(string(&root, "html_url")?)?;
        if notes != format!("{REPOSITORY}/releases/tag/{tag}") {
            return Err(invalid("Invalid release identity."));
        }
        if version <= current {
            return Ok(None);
        }
        let assets = prop(&root, "assets")?
            .as_array()
            .ok_or_else(|| invalid("Release assets are not a list."))?;
        let asset = |name: &str| -> Result<(String, u64), UpdateError> {
            let mut matching = Vec::new();
            for a in assets {
                if string(a, "name")? == name {
                    matching.push(a);
                }
            }
            let [a] = matching[..] else {
                return Err(UpdateError::Invalid(format!(
                    "Missing or duplicate official asset: {name}"
                )));
            };
            let size = prop(a, "size")?
                .as_i64()
                .ok_or_else(|| invalid("Invalid official asset."))?;
            let uri = absolute(string(a, "browser_download_url")?)?;
            if uri != format!("{REPOSITORY}/releases/download/{tag}/{name}")
                || size < 1
                || size as u64 > MAX_SETUP_SIZE
            {
                return Err(invalid("Invalid official asset."));
            }
            Ok((uri, size as u64))
        };
        let (setup, size) = asset(&platform.setup_asset())?;
        let (checksums, _) = asset(CHECKSUMS_ASSET)?;
        Ok(Some(Self {
            version,
            tag: tag.to_owned(),
            notes,
            setup,
            size,
            checksums,
        }))
    }
}

/// C# `OfficialUpdates.Checksum`: the single `<64 hex>  <setup asset>` line, uppercased.
pub fn checksum(text: &str, setup_asset: &str) -> Result<String, UpdateError> {
    let mut found = text.split('\n').filter_map(|line| {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (hash, name) = line.split_once("  ")?;
        (name == setup_asset && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_uppercase())
    });
    match (found.next(), found.next()) {
        (Some(hash), None) => Ok(hash),
        _ => Err(invalid("Invalid or missing setup checksum.")),
    }
}

/// C# `VerifyFile`: streams the file through SHA-256 and compares with the uppercase `hash`.
pub fn verify_file(path: &Path, hash: &str) -> Result<(), UpdateError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    if hex_upper(&hasher.finish()) == hash {
        Ok(())
    } else {
        Err(invalid("Setup checksum mismatch."))
    }
}

/// The pure part of C# `InspectSetup`: a Windows executable (`MZ`, valid `PE\0\0` header) built
/// for `machine`. Authenticode and the file-version check are done by the host.
pub fn inspect_pe<R: Read + Seek>(
    file: &mut R,
    machine: u16,
    rid: &str,
) -> Result<(), UpdateError> {
    let length = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    let mut mz = [0u8; 2];
    if length < 64 || file.read_exact(&mut mz).is_err() || u16::from_le_bytes(mz) != 0x5A4D {
        return Err(invalid("Setup is not a Windows executable."));
    }
    file.seek(SeekFrom::Start(0x3C))?;
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf)?;
    let offset = i32::from_le_bytes(buf);
    if offset < 64 || i64::from(offset) > length as i64 - 6 {
        return Err(invalid("Invalid setup header."));
    }
    file.seek(SeekFrom::Start(offset as u64))?;
    file.read_exact(&mut buf)?;
    let mut m = [0u8; 2];
    file.read_exact(&mut m)?;
    if u32::from_le_bytes(buf) != 0x4550 || u16::from_le_bytes(m) != machine {
        return Err(UpdateError::Invalid(format!(
            "Setup architecture does not match {rid}."
        )));
    }
    Ok(())
}

/// C# `RequireAutomaticInstallPath`: portable and legacy-named executables cannot self-update.
pub fn require_automatic_install_path(installed: bool, path: &Path) -> Result<(), UpdateError> {
    if !installed {
        return Err(UpdateError::Io(
            "Portable copy: quit and replace the portable files manually.".into(),
        ));
    }
    let name = path
        .to_str()
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .unwrap_or("");
    if !name.eq_ignore_ascii_case("NabyScape.exe") {
        return Err(UpdateError::Io(format!(
            "This legacy executable cannot install or restore updates automatically. Quit the app, download the official installer from {REPOSITORY}/releases/latest, run it, then open NabyScape.exe before using automatic recovery."
        )));
    }
    Ok(())
}

// ---- transport ----

/// A response to an update request. Redirects are never followed by the transport.
pub struct HttpResponse {
    pub status: u16,
    /// `Location` header, for 3xx.
    pub location: Option<String>,
    pub content_length: Option<u64>,
    pub body: Box<dyn Read + Send>,
}

impl HttpResponse {
    /// A fully buffered response (tests, small documents).
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self {
            status,
            location: None,
            content_length: Some(body.len() as u64),
            body: Box::new(std::io::Cursor::new(body)),
        }
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self {
            location: Some(location.to_owned()),
            ..Self::new(status, Vec::new())
        }
    }
}

/// An injected GET: `Err` is a transport failure (offline, timeout).
pub type Get<'a> = &'a dyn Fn(&str) -> Result<HttpResponse, String>;

/// The request function plus the pause before the single 429 / 5xx retry.
#[derive(Clone, Copy)]
pub struct Transport<'a> {
    pub get: Get<'a>,
    pub retry_delay: Duration,
}

impl<'a> Transport<'a> {
    pub fn new(get: Get<'a>) -> Self {
        Self {
            get,
            retry_delay: RETRY_DELAY,
        }
    }

    pub fn with_retry_delay(self, retry_delay: Duration) -> Self {
        Self {
            retry_delay,
            ..self
        }
    }
}

impl Transport<'static> {
    /// The real network (see [`default_get`]).
    pub fn real() -> Self {
        Self::new(&default_get)
    }
}

fn update_agent() -> &'static ureq::Agent {
    // Redirects are validated by `get_checked`, so the agent must not follow them itself.
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(crate::http::USER_AGENT)
            .redirects(0)
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()
    })
}

/// The default `get`: no automatic redirects, GitHub JSON accept header, 45 s timeout.
pub fn default_get(url: &str) -> Result<HttpResponse, String> {
    let response = match update_agent()
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(e.to_string()),
    };
    Ok(HttpResponse {
        status: response.status(),
        location: response.header("Location").map(str::to_owned),
        content_length: response
            .header("Content-Length")
            .and_then(|v| v.parse().ok()),
        body: response.into_reader(),
    })
}

/// A GitHub release download redirects once, to its signed asset CDN. Only that is accepted.
fn trusted_redirect(from: &str, next: &str) -> bool {
    let (Ok(from), Ok(next)) = (Url::parse(from), Url::parse(next)) else {
        return false;
    };
    from.host_str() == Some("github.com")
        && next.scheme() == "https"
        && next.host_str() == Some(REDIRECT_HOST)
        && next.username().is_empty()
        && next.password().is_none()
        && next.port().is_none()
}

/// C# `Get`: one trusted redirect hop; one retry after `retry_delay` on 429 / 5xx; non-2xx fails.
fn get_checked(url: &str, transport: Transport<'_>) -> Result<HttpResponse, UpdateError> {
    let Transport { get, retry_delay } = transport;
    for attempt in 0.. {
        let mut response = get(url).map_err(UpdateError::Network)?;
        if matches!(response.status, 302 | 307) {
            let next = response.location.take().unwrap_or_default();
            if !trusted_redirect(url, &next) {
                return Err(invalid("Untrusted update redirect."));
            }
            response = get(&next).map_err(UpdateError::Network)?;
        }
        if (response.status == 429 || response.status >= 500) && attempt < 1 {
            std::thread::sleep(retry_delay);
            continue;
        }
        if !(200..300).contains(&response.status) {
            return Err(UpdateError::Network(format!(
                "Response status code does not indicate success ({}).",
                response.status
            )));
        }
        return Ok(response);
    }
    unreachable!("the loop returns on its second iteration")
}

fn read_text(response: HttpResponse, limit: usize) -> Result<String, UpdateError> {
    let mut bytes = Vec::new();
    response
        .body
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if bytes.len() > limit {
        return Err(invalid("Update metadata is too large."));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ---- check ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// "You have the latest stable version."
    UpToDate,
    Available(Release),
    /// Offline, rate limited, rejected metadata. The app is unchanged.
    Failed(String),
}

impl UpdateCheck {
    /// The status line, localized with `i18n::format`.
    pub fn status_text(&self, lang: &str) -> String {
        match self {
            Self::UpToDate => "You have the latest stable version.".to_owned(),
            Self::Available(r) => {
                crate::i18n::format(lang, "Version {0} is available.", &[&r.version])
            }
            Self::Failed(m) => crate::i18n::format(lang, "Update failed: {0}", &[m]),
        }
    }
}

/// Checks the official endpoint for a release newer than `current` (an `N.N.N` string; an
/// unparsable one counts as `0.0.0`).
pub fn check(current: &str, get: Get<'_>) -> UpdateCheck {
    let platform = Platform::current().unwrap_or(Platform::X64);
    check_with(
        current.parse().unwrap_or(Version::new(0, 0, 0)),
        &platform,
        Transport::new(get),
    )
}

/// [`check`] with an explicit platform and transport.
pub fn check_with(current: Version, platform: &Platform, transport: Transport<'_>) -> UpdateCheck {
    let result = get_checked(ENDPOINT, transport)
        .and_then(|r| read_text(r, METADATA_LIMIT))
        .and_then(|json| Release::parse(&json, current, platform));
    match result {
        Ok(Some(release)) => UpdateCheck::Available(release),
        Ok(None) => UpdateCheck::UpToDate,
        Err(e) => UpdateCheck::Failed(e.to_string()),
    }
}

/// The scheduled entry point: `None` when throttled or disabled (nothing requested, state
/// untouched); otherwise records the attempt in `state` (persist it under [`DOCUMENT_NAME`])
/// and returns the outcome.
pub fn check_scheduled(
    state: &mut UpdateState,
    manual: bool,
    enabled: bool,
    now: DateTime<Utc>,
    current: Version,
    platform: &Platform,
    transport: Transport<'_>,
) -> Option<UpdateCheck> {
    if !check_due(state, manual, enabled, now) {
        return None;
    }
    state.last_attempt = now;
    Some(check_with(current, platform, transport))
}

// ---- download ----

/// A downloaded, checksummed and inspected setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSetup {
    pub path: PathBuf,
    pub hash: String,
}

/// Downloads and verifies the setup for `release` into `staging`.
///
/// Mirrors C# `Download`: fetch `SHA256SUMS`, reuse an already staged setup that verifies,
/// otherwise stream to `NabyScape-Setup.exe.partial` (length must match the release metadata
/// exactly), verify the checksum, run `inspect` (PE/signature/version), then move into place.
/// The partial file never survives, whatever the outcome. `progress` receives whole percents;
/// `cancel` is polled between chunks.
pub fn download_setup(
    release: &Release,
    platform: &Platform,
    staging: &Path,
    transport: Transport<'_>,
    inspect: &dyn Fn(&Path, &Version) -> Result<(), UpdateError>,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<VerifiedSetup, UpdateError> {
    fs::create_dir_all(staging)?;
    let partial = staging.join(format!("{SETUP_FILE}.partial"));
    let result = download_inner(
        release, platform, staging, transport, inspect, progress, cancel,
    );
    let _ = fs::remove_file(&partial);
    result
}

fn download_inner(
    release: &Release,
    platform: &Platform,
    staging: &Path,
    transport: Transport<'_>,
    inspect: &dyn Fn(&Path, &Version) -> Result<(), UpdateError>,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<VerifiedSetup, UpdateError> {
    let partial = staging.join(format!("{SETUP_FILE}.partial"));
    let target = staging.join(SETUP_FILE);
    let (partial, target) = (partial.as_path(), target.as_path());
    let cancelled = || cancel.load(AtomicOrdering::SeqCst);
    let sums = read_text(get_checked(&release.checksums, transport)?, CHECKSUM_LIMIT)?;
    let hash = checksum(&sums, &platform.setup_asset())?;
    if fs::metadata(target).is_ok_and(|m| m.len() == release.size) {
        let reuse = verify_file(target, &hash).and_then(|()| inspect(target, &release.version));
        match reuse {
            Ok(()) => {
                if cancelled() {
                    return Err(UpdateError::Cancelled);
                }
                return Ok(VerifiedSetup {
                    path: target.to_owned(),
                    hash,
                });
            }
            // A stale or incomplete cached setup is replaced below.
            Err(UpdateError::Invalid(_)) => {}
            Err(e) => return Err(e),
        }
    }
    let response = get_checked(&release.setup, transport)?;
    if response.content_length.is_some_and(|l| l != release.size) {
        return Err(invalid("Setup length does not match release metadata."));
    }
    let mut input = response.body;
    let mut output = File::create(partial)?;
    let mut buffer = vec![0u8; 80 * 1024];
    let (mut total, mut last_percent) = (0u64, u64::MAX);
    loop {
        if cancelled() {
            return Err(UpdateError::Cancelled);
        }
        let n = input
            .read(&mut buffer)
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > release.size {
            return Err(invalid("Setup is larger than expected."));
        }
        output.write_all(&buffer[..n])?;
        let percent = total * 100 / release.size;
        if percent != last_percent {
            last_percent = percent;
            progress(percent);
        }
    }
    if total != release.size {
        return Err(invalid("Setup download was interrupted."));
    }
    output.flush()?;
    drop(output);
    if cancelled() {
        return Err(UpdateError::Cancelled);
    }
    verify_file(partial, &hash)?;
    inspect(partial, &release.version)?;
    if cancelled() {
        return Err(UpdateError::Cancelled);
    }
    #[cfg(windows)]
    fs::write(
        format!("{}:Zone.Identifier", partial.display()),
        format!(
            "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={}\r\n",
            release.setup
        ),
    )?;
    fs::rename(partial, target)?;
    Ok(VerifiedSetup {
        path: target.to_owned(),
        hash,
    })
}
