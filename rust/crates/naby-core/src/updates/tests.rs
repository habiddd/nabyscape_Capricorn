use super::runtime::*;
use super::*;
use crate::settings::Settings;
use chrono::TimeZone;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::io::Cursor;

const ASSET: &str = "NabyScape-win-x64-Setup.exe";
const X64: Platform = Platform::X64;

fn release_json(version: &str, host: &str, name: &str) -> String {
    json!({
        "tag_name": format!("v{version}"),
        "html_url": format!("{host}/releases/tag/v{version}"),
        "draft": false,
        "prerelease": false,
        "assets": [
            {"name": name, "browser_download_url": format!("{host}/releases/download/v{version}/{name}"), "size": 4},
            {"name": "SHA256SUMS", "browser_download_url": format!("{host}/releases/download/v{version}/SHA256SUMS"), "size": 100},
        ],
    })
    .to_string()
}

fn release(version: &str) -> String {
    release_json(version, REPOSITORY, ASSET)
}

fn v(a: u32, b: u32, c: u32) -> Version {
    Version::new(a, b, c)
}

fn parse(json: &str, current: Version) -> Result<Option<Release>, UpdateError> {
    Release::parse(json, current, &X64)
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
}

fn fast(get: Get<'_>) -> Transport<'_> {
    Transport::new(get).with_retry_delay(Duration::ZERO)
}

#[test]
fn semver_ordering() {
    assert_eq!(compare_versions("2.4.3", "2.4.2"), Some(Ordering::Greater));
    assert_eq!(compare_versions("2.4.3", "2.4.3"), Some(Ordering::Equal));
    assert_eq!(compare_versions("2.4.3", "3.0.0"), Some(Ordering::Less));
    // Numeric, not lexical.
    assert_eq!(compare_versions("2.10.0", "2.9.9"), Some(Ordering::Greater));
    assert_eq!(
        compare_versions("10.0.0", "9.99.99"),
        Some(Ordering::Greater)
    );
    assert_eq!(compare_versions("1.0.0", "1.0"), None);
    assert_eq!(compare_versions("1.0.0-beta", "1.0.0"), None);
    assert_eq!(compare_versions("", "1.0.0"), None);
    assert_eq!("2.24.0".parse::<Version>().unwrap().to_string(), "2.24.0");
    for bad in [
        "1",
        "1.2",
        "1.2.3.4",
        "a.b.c",
        "1.2.",
        ".1.2",
        "1.2.3-beta",
        "-1.2.3",
        "99999999999.0.0",
        "1.2.3 ",
    ] {
        assert!(bad.parse::<Version>().is_err(), "{bad}");
    }
    assert!(Version::installed() > v(0, 0, 0));
}

#[test]
fn updates_accept_newer_stable_and_ignore_equal_or_older() {
    let r = parse(&release("2.4.3"), v(2, 4, 2)).unwrap().unwrap();
    assert_eq!(r.version, v(2, 4, 3));
    assert_eq!(r.tag, "v2.4.3");
    assert_eq!(r.notes, format!("{REPOSITORY}/releases/tag/v2.4.3"));
    assert_eq!(
        r.setup,
        format!("{REPOSITORY}/releases/download/v2.4.3/{ASSET}")
    );
    assert_eq!(
        r.checksums,
        format!("{REPOSITORY}/releases/download/v2.4.3/SHA256SUMS")
    );
    assert_eq!(r.size, 4);
    assert_eq!(parse(&release("2.4.3"), v(2, 4, 3)).unwrap(), None);
    assert_eq!(parse(&release("2.4.3"), v(3, 0, 0)).unwrap(), None);
    // An older release needs no assets at all.
    let bare = json!({"tag_name":"v1.0.0","html_url":format!("{REPOSITORY}/releases/tag/v1.0.0"),"draft":false,"prerelease":false}).to_string();
    assert_eq!(parse(&bare, v(2, 0, 0)).unwrap(), None);
}

#[test]
fn updates_reject_malformed_host_schema_prerelease_and_wrong_architecture() {
    let cur = v(2, 4, 2);
    let bad = |json: String| assert!(parse(&json, cur).is_err(), "{json}");
    bad("{}".into());
    bad(String::new());
    bad("[]".into());
    bad(release_json(
        "2.4.3",
        "http://github.com/BillyNabil/nabyscape",
        ASSET,
    ));
    bad(release_json(
        "2.4.3",
        "https://github.com.evil/BillyNabil/nabyscape",
        ASSET,
    ));
    bad(release_json(
        "2.4.3",
        "https://github.com/BillyNabil/nabyscape-evil",
        ASSET,
    ));
    bad(release_json(
        "2.4.3",
        REPOSITORY,
        "NabyScape-win-arm64-Setup.exe",
    ));
    bad(release("2.4.3").replace("\"prerelease\":false", "\"prerelease\":true"));
    bad(release("2.4.3").replace("\"draft\":false", "\"draft\":true"));
    bad(release("2.4.3-beta"));
    bad(release("2.4"));
    bad(release("2.4.3").replace("\"tag_name\":\"v2.4.3\"", "\"tag_name\":\"2.4.3\""));
    // Wrong release identity (notes URL for another tag).
    bad(release("2.4.3").replace("/releases/tag/v2.4.3", "/releases/tag/v2.4.4"));
    // Asset URL outside the release.
    bad(release("2.4.3").replace(
        "/releases/download/v2.4.3/SHA256SUMS",
        "/releases/download/v2.4.4/SHA256SUMS",
    ));
    // Sizes: zero, negative and too large.
    for size in ["0", "-5", "536870913"] {
        bad(release("2.4.3").replacen("\"size\":4", &format!("\"size\":{size}"), 1));
    }
    // Missing checksum asset and duplicate setup asset.
    let mut value: Value = serde_json::from_str(&release("2.4.3")).unwrap();
    let assets = value["assets"].as_array().unwrap().clone();
    value["assets"] = json!([assets[0]]);
    bad(value.to_string());
    value["assets"] = json!([assets[0], assets[0], assets[1]]);
    bad(value.to_string());
    value["assets"] = json!("nope");
    bad(value.to_string());
    // The arm64 platform wants its own asset.
    let arm = release_json("2.4.3", REPOSITORY, "NabyScape-win-arm64-Setup.exe");
    assert!(Release::parse(&arm, cur, &Platform::ARM64)
        .unwrap()
        .is_some());
    assert_eq!(
        Platform::ARM64.setup_asset(),
        "NabyScape-win-arm64-Setup.exe"
    );
}

#[test]
fn checksum_parsing_is_strict() {
    let hash = "A".repeat(64);
    let line = format!("{hash}  {ASSET}");
    assert_eq!(checksum(&line, ASSET).unwrap(), hash);
    assert_eq!(
        checksum(
            &format!("{}  {ASSET}\r\nignored line\r\n", "a".repeat(64)),
            ASSET
        )
        .unwrap(),
        hash
    );
    assert!(checksum(&format!("{line}\n{line}"), ASSET).is_err());
    assert!(checksum("bad", ASSET).is_err());
    assert!(checksum("", ASSET).is_err());
    assert!(checksum(&format!("{}  {ASSET}", "A".repeat(63)), ASSET).is_err());
    assert!(checksum(&format!("{}  {ASSET}", "G".repeat(64)), ASSET).is_err());
    assert!(checksum(&format!("{hash} {ASSET}"), ASSET).is_err());
    assert!(checksum(&format!("{hash}  other.exe"), ASSET).is_err());
    assert!(checksum(&format!("{hash}  {ASSET}x"), ASSET).is_err());
    assert!(checksum(&format!("{hash}  *{ASSET}"), ASSET).is_err());
}

#[test]
fn sha256_known_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
    );
    assert_eq!(
        sha256_hex(b"abcd"),
        "88D4266FD4E6338D13B845FCF289579D209C897823B9217DA3E161936F031589"
    );
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248D6A61D20638B8E5C026930C3E6039A33CE45964FF2167F6ECEDD419DB06C1"
    );
    // Streaming in odd chunks equals one shot, across the 64-byte block boundary.
    let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let mut h = Sha256::new();
    for chunk in data.chunks(37) {
        h.update(chunk);
    }
    assert_eq!(hex_upper(&h.finish()), sha256_hex(&data));
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        sha256_hex(&million),
        "CDC76E5C9914FB9281A1C7E284D73E67F1809A48A497200E046D39CCC7112CD0"
    );
}

fn json_get(body: String) -> impl Fn(&str) -> Result<HttpResponse, String> {
    move |_| Ok(HttpResponse::new(200, body.clone()))
}

#[test]
fn check_reports_available_current_and_failed() {
    let get = json_get(release("9.9.9"));
    match check_with(v(2, 4, 2), &X64, fast(&get)) {
        UpdateCheck::Available(r) => assert_eq!(r.version, v(9, 9, 9)),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        check_with(v(9, 9, 9), &X64, fast(&get)),
        UpdateCheck::UpToDate
    );
    assert_eq!(
        UpdateCheck::UpToDate.status_text("en"),
        "You have the latest stable version."
    );
    let r = parse(&release("9.9.9"), v(1, 0, 0)).unwrap().unwrap();
    assert_eq!(
        UpdateCheck::Available(r.clone()).status_text("en"),
        "Version 9.9.9 is available."
    );
    assert_eq!(
        UpdateCheck::Available(r).status_text("es"),
        "La versión 9.9.9 está disponible."
    );
    let junk = json_get("<html>".into());
    assert!(
        matches!(check_with(v(1, 0, 0), &X64, fast(&junk)), UpdateCheck::Failed(m) if !m.is_empty())
    );
    // The public entry point uses the running architecture and a parsed version string.
    let get = json_get(release("9.9.9"));
    let wanted = Platform::current().unwrap_or(X64).setup_asset();
    let body = release_json("9.9.9", REPOSITORY, &wanted);
    let get2 = json_get(body);
    assert!(matches!(check("2.24.0", &get2), UpdateCheck::Available(_)));
    assert!(matches!(check("garbage", &get2), UpdateCheck::Available(_)));
    let _ = get;
    assert!(UpdateCheck::Failed("x".into())
        .status_text("en")
        .starts_with("Update failed: x"));
}

#[test]
fn updates_offline_timeout_rate_limit_and_redirects_leave_app_intact() {
    for mode in [
        "offline",
        "timeout",
        "rate",
        "redirect",
        "server",
        "not-found",
    ] {
        let calls = Cell::new(0);
        let get = |_: &str| {
            calls.set(calls.get() + 1);
            match mode {
                "offline" => Err("offline".to_owned()),
                "timeout" => Err("timed out".to_owned()),
                "redirect" => Ok(HttpResponse::redirect(302, "https://evil.test/setup.exe")),
                "rate" => Ok(HttpResponse::new(429, "")),
                "server" => Ok(HttpResponse::new(503, "")),
                _ => Ok(HttpResponse::new(404, "")),
            }
        };
        let outcome = check_with(v(2, 4, 2), &X64, fast(&get));
        assert!(matches!(outcome, UpdateCheck::Failed(_)), "{mode}");
        assert!(calls.get() <= 2, "{mode}: {}", calls.get());
    }
}

#[test]
fn rate_limited_check_retries_once_then_succeeds() {
    let calls = Cell::new(0);
    let get = |_: &str| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            Ok(HttpResponse::new(429, ""))
        } else {
            Ok(HttpResponse::new(200, release("9.9.9")))
        }
    };
    assert!(matches!(
        check_with(v(1, 0, 0), &X64, fast(&get)),
        UpdateCheck::Available(_)
    ));
    assert_eq!(calls.get(), 2);
}

#[test]
fn redirect_policy_trusts_only_the_release_asset_cdn() {
    let dl = format!("{REPOSITORY}/releases/download/v1.0.0/x");
    let ok =
        "https://release-assets.githubusercontent.com/github-production-release-asset/1?sig=abc";
    assert!(trusted_redirect(&dl, ok));
    for bad in [
        "http://release-assets.githubusercontent.com/x",
        "https://evil.test/x",
        "https://release-assets.githubusercontent.com.evil.test/x",
        "https://user:pw@release-assets.githubusercontent.com/x",
        "https://user@release-assets.githubusercontent.com/x",
        "https://release-assets.githubusercontent.com:8443/x",
        "https://objects.githubusercontent.com/x",
        "/relative",
        "",
    ] {
        assert!(!trusted_redirect(&dl, bad), "{bad}");
    }
    // Only github.com may redirect (the API endpoint may not).
    assert!(!trusted_redirect(ENDPOINT, ok));
    assert!(!trusted_redirect("https://evil.test/x", ok));
}

// ---- download ----

struct Net {
    setup: Vec<u8>,
    sums: String,
    setup_calls: Cell<usize>,
    redirect: bool,
    lie_length: Option<u64>,
}

impl Net {
    fn new(setup: &[u8], hash: &str) -> Self {
        Self {
            setup: setup.to_vec(),
            sums: format!("{hash}  {ASSET}"),
            setup_calls: Cell::new(0),
            redirect: true,
            lie_length: None,
        }
    }

    fn get(&self, url: &str) -> Result<HttpResponse, String> {
        if url.ends_with("SHA256SUMS") {
            return Ok(HttpResponse::new(200, self.sums.clone()));
        }
        if url.starts_with(REPOSITORY) && self.redirect {
            return Ok(HttpResponse::redirect(
                302,
                "https://release-assets.githubusercontent.com/a?sig=1",
            ));
        }
        self.setup_calls.set(self.setup_calls.get() + 1);
        let mut response = HttpResponse::new(200, self.setup.clone());
        if let Some(l) = self.lie_length {
            response.content_length = Some(l);
        }
        Ok(response)
    }
}

fn release_of(size: u64) -> Release {
    let mut r = parse(&release("9.9.9"), v(1, 0, 0)).unwrap().unwrap();
    r.size = size;
    r
}

fn staging() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn run_download(
    net: &Net,
    dir: &Path,
    release: &Release,
    inspect: &dyn Fn(&Path, &Version) -> Result<(), UpdateError>,
    percents: &RefCell<Vec<u64>>,
) -> Result<VerifiedSetup, UpdateError> {
    let get = |u: &str| net.get(u);
    download_setup(
        release,
        &X64,
        dir,
        fast(&get),
        inspect,
        &mut |p| percents.borrow_mut().push(p),
        &AtomicBool::new(false),
    )
}

#[test]
fn updates_bad_checksum_and_truncated_downloads_never_become_executable() {
    for (served, expected_len) in [(&b"abcd"[..], 4u64), (&b"abc"[..], 4)] {
        let dir = staging();
        // The published hash is for different content than what is served.
        let net = Net::new(served, &"A".repeat(64));
        let result = run_download(
            &net,
            dir.path(),
            &release_of(expected_len),
            &|_, _| Ok(()),
            &RefCell::new(vec![]),
        );
        assert!(result.is_err());
        assert!(!dir.path().join("NabyScape-Setup.exe").exists());
        assert!(!dir.path().join("NabyScape-Setup.exe.partial").exists());
    }
}

#[test]
fn updates_successful_download_verifies_inspects_and_reuses_the_staged_file() {
    let dir = staging();
    let hash = sha256_hex(b"abcd");
    let net = Net::new(b"abcd", &hash);
    let inspected = Cell::new(0);
    let percents = RefCell::new(vec![]);
    let inspect = |path: &Path, version: &Version| {
        inspected.set(inspected.get() + 1);
        assert_eq!(*version, v(9, 9, 9));
        assert!(path.exists());
        Ok(())
    };
    let done = run_download(&net, dir.path(), &release_of(4), &inspect, &percents).unwrap();
    assert_eq!(done.hash, hash);
    assert_eq!(done.path, dir.path().join("NabyScape-Setup.exe"));
    assert_eq!(fs::read(&done.path).unwrap(), b"abcd");
    assert_eq!((inspected.get(), net.setup_calls.get()), (1, 1));
    assert_eq!(*percents.borrow(), [100]);
    assert!(!dir.path().join("NabyScape-Setup.exe.partial").exists());
    // A second download reuses the verified staged setup without requesting it again.
    let again = run_download(&net, dir.path(), &release_of(4), &inspect, &percents).unwrap();
    assert_eq!(again, done);
    assert_eq!((inspected.get(), net.setup_calls.get()), (2, 1));
    // A stale staged file of the right size but wrong content is replaced.
    fs::write(&done.path, b"xxxx").unwrap();
    run_download(&net, dir.path(), &release_of(4), &inspect, &percents).unwrap();
    assert_eq!(fs::read(&done.path).unwrap(), b"abcd");
    assert_eq!(net.setup_calls.get(), 2);
}

#[test]
fn download_inspection_failure_discards_the_file() {
    let dir = staging();
    let net = Net::new(b"abcd", &sha256_hex(b"abcd"));
    let result = run_download(
        &net,
        dir.path(),
        &release_of(4),
        &|_, _| {
            Err(UpdateError::Invalid(
                "Setup version does not match the release.".into(),
            ))
        },
        &RefCell::new(vec![]),
    );
    assert_eq!(
        result.unwrap_err().to_string(),
        "Setup version does not match the release."
    );
    assert!(!dir.path().join("NabyScape-Setup.exe").exists());
    assert!(!dir.path().join("NabyScape-Setup.exe.partial").exists());
}

#[test]
fn download_rejects_wrong_lengths_and_untrusted_redirects() {
    let hash = sha256_hex(b"abcd");
    // Declared length differs from the release metadata.
    let dir = staging();
    let mut net = Net::new(b"abcd", &hash);
    net.lie_length = Some(5);
    let err = run_download(
        &net,
        dir.path(),
        &release_of(4),
        &|_, _| Ok(()),
        &RefCell::new(vec![]),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Setup length does not match release metadata."
    );
    // More bytes than declared, with no length header to catch it up front.
    let dir = staging();
    let mut net = Net::new(b"abcdef", &hash);
    net.lie_length = Some(4);
    let get = |u: &str| {
        let mut r = net.get(u)?;
        if !u.ends_with("SHA256SUMS") {
            r.content_length = None;
        }
        Ok(r)
    };
    let err = download_setup(
        &release_of(4),
        &X64,
        dir.path(),
        fast(&get),
        &|_, _| Ok(()),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Setup is larger than expected.");
    assert!(!dir.path().join("NabyScape-Setup.exe.partial").exists());
    // Redirect to an untrusted host.
    let dir = staging();
    let get = |u: &str| {
        if u.ends_with("SHA256SUMS") {
            Ok(HttpResponse::new(200, format!("{hash}  {ASSET}")))
        } else {
            Ok(HttpResponse::redirect(307, "https://evil.test/setup.exe"))
        }
    };
    let err = download_setup(
        &release_of(4),
        &X64,
        dir.path(),
        fast(&get),
        &|_, _| Ok(()),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Untrusted update redirect.");
}

#[test]
fn download_cancel_stops_and_cleans_up() {
    let dir = staging();
    let net = Net::new(b"abcd", &sha256_hex(b"abcd"));
    let get = |u: &str| net.get(u);
    let err = download_setup(
        &release_of(4),
        &X64,
        dir.path(),
        fast(&get),
        &|_, _| Ok(()),
        &mut |_| {},
        &AtomicBool::new(true),
    )
    .unwrap_err();
    assert_eq!(err, UpdateError::Cancelled);
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[test]
fn download_checksum_asset_errors_surface() {
    let dir = staging();
    let net = Net::new(b"abcd", "not-a-hash");
    let err = run_download(
        &net,
        dir.path(),
        &release_of(4),
        &|_, _| Ok(()),
        &RefCell::new(vec![]),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Invalid or missing setup checksum.");
    // Oversized metadata is rejected rather than buffered.
    let get = |u: &str| {
        Ok(HttpResponse::new(
            200,
            vec![
                b'x';
                if u.ends_with("SHA256SUMS") {
                    70_000
                } else {
                    10
                }
            ],
        ))
    };
    let err = download_setup(
        &release_of(4),
        &X64,
        dir.path(),
        fast(&get),
        &|_, _| Ok(()),
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Update metadata is too large.");
}

#[test]
fn verify_file_compares_hashes() {
    let dir = staging();
    let path = dir.path().join("f");
    fs::write(&path, b"abcd").unwrap();
    assert!(verify_file(&path, &sha256_hex(b"abcd")).is_ok());
    assert!(verify_file(&path, &"0".repeat(64)).is_err());
    assert!(matches!(
        verify_file(&dir.path().join("missing"), "x"),
        Err(UpdateError::Io(_))
    ));
}

// ---- PE inspection and install path ----

fn pe(machine: u16, offset: i32) -> Vec<u8> {
    let mut bytes = vec![0u8; 256];
    bytes[0] = b'M';
    bytes[1] = b'Z';
    bytes[0x3C..0x40].copy_from_slice(&offset.to_le_bytes());
    let o = offset.max(0) as usize;
    if o >= 64 && o + 6 <= bytes.len() {
        bytes[o..o + 4].copy_from_slice(&0x4550u32.to_le_bytes());
        bytes[o + 4..o + 6].copy_from_slice(&machine.to_le_bytes());
    }
    bytes
}

#[test]
fn inspect_pe_checks_header_and_architecture() {
    let check = |bytes: Vec<u8>| inspect_pe(&mut Cursor::new(bytes), 0x8664, "win-x64");
    assert!(check(pe(0x8664, 128)).is_ok());
    assert_eq!(
        check(pe(0xAA64, 128)).unwrap_err().to_string(),
        "Setup architecture does not match win-x64."
    );
    assert_eq!(
        check(vec![0; 10]).unwrap_err().to_string(),
        "Setup is not a Windows executable."
    );
    let mut not_mz = pe(0x8664, 128);
    not_mz[0] = b'X';
    assert_eq!(
        check(not_mz).unwrap_err().to_string(),
        "Setup is not a Windows executable."
    );
    assert_eq!(
        check(pe(0x8664, 10)).unwrap_err().to_string(),
        "Invalid setup header."
    );
    assert_eq!(
        check(pe(0x8664, -1)).unwrap_err().to_string(),
        "Invalid setup header."
    );
    assert_eq!(
        check(pe(0x8664, 255)).unwrap_err().to_string(),
        "Invalid setup header."
    );
    let mut bad_sig = pe(0x8664, 128);
    bad_sig[128] = 0;
    assert!(check(bad_sig).is_err());
}

#[test]
fn install_path_guard() {
    let ok = require_automatic_install_path(
        true,
        Path::new("C:\\Program Files\\NabyScape\\NabyScape.exe"),
    );
    assert!(ok.is_ok());
    assert!(require_automatic_install_path(true, Path::new("C:\\x\\nabyscape.EXE")).is_ok());
    let portable =
        require_automatic_install_path(false, Path::new("C:\\x\\NabyScape.exe")).unwrap_err();
    assert!(portable.to_string().starts_with("Portable copy"));
    let legacy =
        require_automatic_install_path(true, Path::new("C:\\x\\LumaIsland.exe")).unwrap_err();
    assert!(
        legacy.to_string().contains("legacy executable") && legacy.to_string().contains(REPOSITORY)
    );
}

// ---- scheduling and state ----

#[test]
fn updates_24_hour_state_does_not_poll() {
    let mut state = UpdateState::default();
    let calls = Cell::new(0);
    let get = |_: &str| {
        calls.set(calls.get() + 1);
        Ok(HttpResponse::new(200, release("9.9.9")))
    };
    let run = |state: &mut UpdateState, manual, enabled, at| {
        check_scheduled(state, manual, enabled, at, v(2, 4, 2), &X64, fast(&get))
    };
    // Disabled automatic checks never run; manual ones always do.
    assert!(run(&mut state, false, false, now()).is_none() && calls.get() == 0);
    assert!(matches!(
        run(&mut state, true, false, now()),
        Some(UpdateCheck::Available(_))
    ));
    assert_eq!((calls.get(), state.last_attempt), (1, now()));
    // Within 24 h: skipped. After: runs.
    let soon = now() + ChronoDuration::hours(23);
    assert!(run(&mut state, false, true, soon).is_none());
    assert_eq!((calls.get(), state.last_attempt), (1, now()));
    let later = now() + ChronoDuration::hours(24);
    assert!(run(&mut state, false, true, later).is_some());
    assert_eq!((calls.get(), state.last_attempt), (2, later));
    // A fresh install (never attempted) checks immediately.
    let mut fresh = UpdateState::default();
    assert!(check_due(&fresh, false, true, now()));
    fresh.last_attempt = now();
    assert!(!check_due(&fresh, false, true, now()));
}

#[test]
fn failed_scheduled_check_still_records_the_attempt() {
    let mut state = UpdateState::default();
    let get = |_: &str| Err("offline".to_owned());
    let out = check_scheduled(&mut state, false, true, now(), v(1, 0, 0), &X64, fast(&get));
    assert!(matches!(out, Some(UpdateCheck::Failed(_))));
    assert_eq!(state.last_attempt, now());
}

#[test]
fn update_state_document_matches_csharp() {
    assert_eq!(DOCUMENT_NAME, "update-state");
    let state = UpdateState {
        last_attempt: now(),
    };
    let json = serde_json::to_string(&state).unwrap();
    assert!(json.starts_with("{\"LastAttempt\":\"2026-10-29T12:00:00"));
    assert_eq!(serde_json::from_str::<UpdateState>(&json).unwrap(), state);
    let from_csharp: UpdateState =
        serde_json::from_str("{\"LastAttempt\":\"2026-10-29T14:00:00+02:00\"}").unwrap();
    assert_eq!(from_csharp.last_attempt, now());
    assert_eq!(
        serde_json::from_str::<UpdateState>("{}").unwrap(),
        UpdateState::default()
    );
}

// ---- runtime.rs ----

#[test]
fn runtime_change_flags() {
    let both = RuntimeChange::MEDIA | RuntimeChange::SETTINGS;
    assert_eq!(both.0, 130);
    assert!(both.intersects(RuntimeChange::MEDIA) && both.contains(RuntimeChange::SETTINGS));
    assert!(!both.intersects(RuntimeChange::CLOCK) && !both.contains(RuntimeChange::ALL));
    let mut all = RuntimeChange::default();
    for f in [
        RuntimeChange::CLOCK,
        RuntimeChange::MEDIA,
        RuntimeChange::AUDIO,
        RuntimeChange::COUNTDOWN,
        RuntimeChange::PRODUCTIVITY,
        RuntimeChange::CALENDAR,
        RuntimeChange::SYSTEM,
        RuntimeChange::SETTINGS,
    ] {
        all = all | f;
    }
    assert_eq!(all, RuntimeChange::ALL);
}

#[test]
fn media_sample_gate_limits_to_thirty_hertz_per_source() {
    let mut gate = MediaSampleGate::new(1000.0);
    assert!(gate.accept(Some("a"), 0));
    assert!(!gate.accept(Some("a"), 10), "inside the 33 ms lease");
    assert!(!gate.accept(Some("a"), 33));
    assert!(gate.accept(Some("a"), 34));
    assert_eq!(gate.sequence(), 2);
    // A different source is accepted immediately.
    assert!(gate.accept(Some("b"), 35));
    assert!(gate.accept(None, 36));
    // A stamp going backwards restarts the lease.
    assert!(gate.accept(None, 5));
    // Late samples catch the cadence up instead of bursting.
    let mut gate = MediaSampleGate::new(3000.0);
    assert!(gate.accept(Some("a"), 0));
    assert!(gate.accept(Some("a"), 150));
    assert!(!gate.accept(Some("a"), 160));
    gate.reset();
    assert!(gate.accept(Some("a"), 161));
}

#[test]
fn media_features_wanted_rule() {
    let mut s = Settings {
        media_integration_enabled: false,
        ..Settings::default()
    };
    assert!(!media_features_wanted(&s));
    s.media_integration_enabled = true;
    for w in &mut s.widgets {
        w.enabled = w.id == "media";
    }
    assert!(media_features_wanted(&s));
    for w in &mut s.widgets {
        w.enabled = false;
    }
    s.hover_follows_widgets = false;
    assert_eq!(
        media_features_wanted(&s),
        crate::settings::policy::hover_pages(&s)
            .iter()
            .any(|p| p == "media")
    );
}
