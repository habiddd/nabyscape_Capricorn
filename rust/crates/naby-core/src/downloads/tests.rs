use super::*;
use chrono::TimeZone;
use std::collections::BTreeMap;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
}

fn at(secs: i64) -> DateTime<Utc> {
    t0() + Duration::milliseconds(secs * 500)
}

fn sig(size: u64) -> FileSig {
    FileSig {
        size,
        modified_ms: 1,
    }
}

#[derive(Default)]
struct Fs {
    files: BTreeMap<String, Probe>,
    exists: bool,
    list_error: Option<String>,
    listed: usize,
}

impl Fs {
    fn new() -> Self {
        Self {
            exists: true,
            ..Self::default()
        }
    }
    fn put(&mut self, name: &str, size: u64) {
        self.files.insert(name.to_owned(), Probe::Ready(sig(size)));
    }
}

impl DownloadFs for Fs {
    fn list(&mut self) -> Result<Vec<String>, String> {
        self.listed += 1;
        match &self.list_error {
            Some(m) => Err(m.clone()),
            None => Ok(self.files.keys().cloned().collect()),
        }
    }
    fn probe(&mut self, path: &str) -> Probe {
        self.files.get(path).copied().unwrap_or(Probe::Missing)
    }
    fn folder_exists(&mut self) -> bool {
        self.exists
    }
}

fn baseline(fs: &Fs) -> Vec<(String, FileSig)> {
    fs.files
        .iter()
        .filter_map(|(p, v)| match v {
            Probe::Ready(s) | Probe::Locked(s) => Some((p.clone(), *s)),
            _ => None,
        })
        .collect()
}

/// Runs ticks every 500 ms and collects arrivals.
fn run(inf: &mut DownloadInference, fs: &mut Fs, from: i64, to: i64) -> Vec<String> {
    let mut out = Vec::new();
    for step in from..=to {
        out.extend(inf.tick(at(step), fs).unwrap());
    }
    out
}

#[test]
fn final_name_filters_temporary_files() {
    assert!(!final_name("a.part") && !final_name("a.crdownload") && !final_name("~a.tmp"));
    assert!(final_name("a.pdf"));
    assert!(!final_name("C:\\Downloads\\x.PARTIAL"));
    assert!(!final_name("C:\\Downloads\\.hidden") && !final_name("/d/~lock"));
    assert!(!final_name("a.Download") && !final_name("a.opdownload") && !final_name("a.TEMP"));
    assert!(final_name("C:\\Downloads\\.tmp.d\\report.txt"));
    assert!(final_name("noextension") && final_name("archive.tar.gz"));
}

#[test]
fn download_baseline_is_not_an_arrival() {
    let mut fs = Fs::new();
    fs.put("existing.txt", 8);
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl\\", baseline(&fs)).unwrap();
    assert!(inf.running() && inf.folder() == "C:\\dl");
    assert!(run(&mut inf, &mut fs, 0, 20).is_empty());
    assert_eq!(inf.candidates(), 0);
    // Re-listing (reconcile) does not announce the baseline either.
    inf.request_reconcile();
    assert!(run(&mut inf, &mut fs, 21, 30).is_empty());
}

#[test]
fn download_burst_groups() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    for i in 0..12 {
        let name = format!("burst-{i}.txt");
        fs.put(&name, 1);
        inf.on_changed(&name, at(0));
    }
    // Not announced before two stable seconds, then all arrive together, sorted.
    assert!(run(&mut inf, &mut fs, 0, 3).is_empty());
    let batch = inf.tick(at(4), &mut fs).unwrap();
    let mut sorted = batch.clone();
    sorted.sort_by_key(|p| p.to_uppercase());
    assert_eq!(batch, sorted);
    assert_eq!(batch.len(), 12);
    // Each arrives once.
    assert!(run(&mut inf, &mut fs, 6, 20).is_empty());
}

#[test]
fn download_partial_rename_becomes_arrival() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.put("download.crdownload", 9);
    inf.on_changed("download.crdownload", at(0));
    assert!(run(&mut inf, &mut fs, 0, 8).is_empty());
    assert_eq!(inf.candidates(), 0);
    fs.files.remove("download.crdownload");
    fs.put("download.txt", 9);
    inf.on_renamed("download.crdownload", "download.txt", at(9));
    assert_eq!(run(&mut inf, &mut fs, 9, 15), ["download.txt"]);
}

#[test]
fn download_renaming_a_completed_file_is_not_new() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.put("final.txt", 4);
    inf.on_changed("final.txt", at(0));
    assert_eq!(run(&mut inf, &mut fs, 0, 6), ["final.txt"]);
    for i in 0..8 {
        inf.on_changed("final.txt", at(7 + i));
    }
    fs.files.remove("final.txt");
    fs.put("renamed.txt", 4);
    inf.on_renamed("final.txt", "renamed.txt", at(8));
    assert!(run(&mut inf, &mut fs, 8, 20).is_empty());
    // Case-only differences resolve to the same file.
    inf.on_changed("RENAMED.TXT", at(21));
    assert_eq!(inf.candidates(), 0);
}

#[test]
fn download_growing_file_waits_for_stable_size() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.put("growing.txt", 1);
    inf.on_changed("growing.txt", at(0));
    assert!(run(&mut inf, &mut fs, 0, 3).is_empty());
    fs.put("growing.txt", 2);
    // Size changed at step 4: stable clock restarts, so no arrival before step 8.
    assert!(run(&mut inf, &mut fs, 4, 7).is_empty());
    assert_eq!(run(&mut inf, &mut fs, 8, 10), ["growing.txt"]);
}

#[test]
fn download_locked_file_waits_until_readable() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.files.insert("locked.txt".into(), Probe::Locked(sig(1)));
    inf.on_changed("locked.txt", at(0));
    assert!(run(&mut inf, &mut fs, 0, 20).is_empty());
    assert_eq!(inf.candidates(), 1);
    fs.put("locked.txt", 1);
    assert_eq!(run(&mut inf, &mut fs, 21, 22), ["locked.txt"]);
}

#[test]
fn download_locked_candidate_expires_after_attempt_limit() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.files.insert("never.txt".into(), Probe::Locked(sig(1)));
    inf.on_changed("never.txt", at(0));
    run(&mut inf, &mut fs, 0, 300);
    assert_eq!(inf.candidates(), 0);
}

#[test]
fn download_missing_denied_and_cancelled_files_are_dropped() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    inf.on_changed("cancel.txt", at(0));
    fs.files.insert("denied.txt".into(), Probe::Denied);
    inf.on_changed("denied.txt", at(0));
    assert_eq!(inf.candidates(), 2);
    assert!(run(&mut inf, &mut fs, 0, 10).is_empty());
    assert_eq!(inf.candidates(), 0);
    // Deleting a pending candidate removes it immediately.
    fs.put("gone.txt", 1);
    inf.on_changed("gone.txt", at(11));
    inf.on_deleted("gone.txt");
    assert_eq!(inf.candidates(), 0);
}

#[test]
fn download_candidate_capacity_requests_reconcile() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    for i in 0..180 {
        let name = format!("capacity-{i}.txt");
        fs.put(&name, 1);
        inf.on_changed(&name, at(0));
    }
    assert!(inf.candidates() <= MAX_PENDING);
    // The overflow is recovered by a later listing, and nothing is announced twice.
    let mut seen = Vec::new();
    for step in 0..40 {
        seen.extend(inf.tick(at(step), &mut fs).unwrap());
    }
    assert_eq!(seen.len(), 180);
    let unique: std::collections::HashSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 180);
}

#[test]
fn download_disable_and_folder_change_cleanup() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.put("a.txt", 1);
    inf.on_changed("a.txt", at(0));
    assert_eq!(inf.candidates(), 1);
    // Same folder, different case: no restart, candidates kept.
    inf.configure(true, "c:\\DL\\", []).unwrap();
    assert_eq!(inf.candidates(), 1);
    // A different folder restarts with an empty state.
    inf.configure(true, "D:\\other", []).unwrap();
    assert_eq!((inf.candidates(), inf.folder()), (0, "D:\\other"));
    inf.configure(false, "", []).unwrap();
    assert!(!inf.running() && inf.candidates() == 0 && inf.folder().is_empty());
    // Events while disabled are ignored.
    inf.on_changed("b.txt", at(1));
    inf.request_reconcile();
    assert_eq!(inf.candidates(), 0);
    assert!(inf.tick(at(2), &mut fs).unwrap().is_empty());
    assert_eq!(fs.listed, 0);
}

#[test]
fn download_baseline_over_limit_fails_closed() {
    let many = (0..=MAX_KNOWN).map(|i| (format!("f{i}"), sig(1)));
    let mut inf = DownloadInference::new();
    let err = inf.configure(true, "C:\\dl", many).unwrap_err();
    assert_eq!(err, DownloadError::TooManyFiles);
    assert_eq!(
        err.to_string(),
        "This folder has too many files to monitor safely (limit 10,000)."
    );
    assert!(!inf.running());
    let exactly = (0..MAX_KNOWN).map(|i| (format!("f{i}"), sig(1)));
    inf.configure(true, "C:\\dl", exactly).unwrap();
    assert!(inf.running());
}

#[test]
fn download_folder_disappearing_stops_with_message() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.exists = false;
    let err = inf.tick(at(0), &mut fs).unwrap_err();
    assert_eq!(err, DownloadError::Disconnected);
    assert!(err.to_string().starts_with("Download folder disconnected."));
    assert!(!inf.running() && inf.candidates() == 0);
    // Restarting works.
    fs.exists = true;
    inf.configure(true, "C:\\dl", []).unwrap();
    assert!(inf.running());
}

#[test]
fn download_listing_failures_stop_monitoring() {
    let mut fs = Fs::new();
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    fs.list_error = Some("Access is denied.".into());
    assert_eq!(
        inf.tick(at(0), &mut fs).unwrap_err(),
        DownloadError::Io("Access is denied.".into())
    );
    assert!(!inf.running());

    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", []).unwrap();
    let mut fs = Fs::new();
    for i in 0..=MAX_KNOWN {
        fs.put(&format!("f{i}"), 1);
    }
    assert!(inf.tick(at(0), &mut fs).is_err());
    assert!(!inf.running());
}

#[test]
fn download_file_rewritten_after_baseline_is_a_new_arrival() {
    let mut fs = Fs::new();
    fs.put("report.pdf", 10);
    let mut inf = DownloadInference::new();
    inf.configure(true, "C:\\dl", baseline(&fs)).unwrap();
    // Deleted and downloaded again with different contents.
    inf.on_deleted("report.pdf");
    fs.put("report.pdf", 20);
    inf.on_changed("report.pdf", at(0));
    assert_eq!(run(&mut inf, &mut fs, 0, 6), ["report.pdf"]);
}
