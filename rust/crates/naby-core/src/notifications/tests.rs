use super::*;
use chrono::TimeZone;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
}

fn files(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn dl(title: &str, f: &[&str]) -> InternalNotice {
    InternalNotice::with_files(NoticeKind::Downloads, title, "inferred", files(f))
}

#[test]
fn notification_routing() {
    assert_eq!(route("Notch", true), (false, true));
    assert_eq!(route("Notch", false), (true, false));
    assert_eq!(route("Both", false), (true, true));
    assert_eq!(route("Both", true), (false, true));
    assert_eq!(route("Windows", false), (false, true));
    assert_eq!(route("Off", false), (false, false));
    assert_eq!(route("anything else", false), (true, false));
}

#[test]
fn notification_delivery_off_suppresses() {
    let mut s = Settings::default();
    assert!(s.notification_delivery == "Notch" && !s.downloads_enabled);
    let mut m = Notifications::new();
    let now = t0();
    let windows = std::cell::Cell::new(0usize);
    let post = |m: &mut Notifications, s: &Settings, kind: NoticeKind, hidden: bool| {
        let d = m.post(s, InternalNotice::new(kind, "title", "detail"), hidden, now);
        windows.set(windows.get() + d.windows.len());
    };
    post(&mut m, &s, NoticeKind::Timer, true);
    assert!(windows.get() == 1 && m.count() == 0);
    s.notification_delivery = "Both".into();
    post(&mut m, &s, NoticeKind::Timer, false);
    assert!(windows.get() == 2 && m.count() == 1);
    s.notify_timer = false;
    post(&mut m, &s, NoticeKind::Timer, false);
    assert!(m.count() == 1 && windows.get() == 2);
    s.notification_delivery = "Off".into();
    post(&mut m, &s, NoticeKind::Failure, false);
    assert!(m.count() == 1 && windows.get() == 2);
    s.notification_delivery = "Notch".into();
    for _ in 0..20 {
        post(&mut m, &s, NoticeKind::Failure, false);
    }
    assert_eq!(m.count(), 10);
    // Busy overlay keeps notices; an idle one expires the front after 12 s.
    assert!(!m.tick(now + Duration::hours(1), true, 12.0));
    assert_eq!(m.count(), 10);
    assert!(m.tick(
        now + Duration::hours(1) + Duration::seconds(13),
        false,
        12.0
    ));
    assert_eq!(m.count(), 9);
    // Download groups merge within three seconds, not after.
    m.clear();
    m.post(&s, dl("one", &["a"]), false, now);
    m.post(
        &s,
        dl("two", &["a", "b"]),
        false,
        now + Duration::seconds(1),
    );
    assert_eq!(m.count(), 1);
    assert_eq!(
        m.current().unwrap().files.as_deref(),
        Some(&files(&["a", "b"])[..])
    );
    assert_eq!(m.current().unwrap().title, "2 downloads ready");
    m.tick(now + Duration::minutes(1), true, 12.0);
    m.post(
        &s,
        dl("later burst", &["c"]),
        false,
        now + Duration::minutes(1),
    );
    assert_eq!(m.count(), 2);
    // Delivery still reports Windows when both routes are selected.
    s.notification_delivery = "Both".into();
    let d = m.post(
        &s,
        InternalNotice::new(NoticeKind::Failure, "failure", "delivery"),
        false,
        now,
    );
    assert_eq!((m.count(), d.windows.len(), d.changed), (3, 1, true));
}

#[test]
fn notification_disabled_kinds_and_quiet_hours() {
    let mut s = Settings::default();
    let mut m = Notifications::new();
    for (kind, flag) in [
        (NoticeKind::Timer, 0),
        (NoticeKind::Focus, 1),
        (NoticeKind::Downloads, 2),
        (NoticeKind::Updates, 3),
        (NoticeKind::Failure, 4),
    ] {
        match flag {
            0 => s.notify_timer = false,
            1 => s.notify_focus = false,
            2 => s.notify_downloads = false,
            3 => s.notify_updates = false,
            _ => s.notify_failures = false,
        }
        assert!(!enabled(&s, kind));
        let d = m.post(&s, InternalNotice::new(kind, "t", "d"), false, t0());
        assert!(d.windows.is_empty() && !d.changed);
    }
    assert_eq!(m.count(), 0);
    assert_eq!(NoticeKind::parse("updates"), NoticeKind::Updates);
    assert_eq!(NoticeKind::parse("whatever"), NoticeKind::Failure);

    // Snoozed notifications are suppressed on every route.
    let s = Settings {
        notifications_snoozed_until: Some((t0() + Duration::hours(1)).fixed_offset()),
        notification_delivery: "Both".into(),
        ..Settings::default()
    };
    let d = m.post(
        &s,
        InternalNotice::new(NoticeKind::Timer, "t", "d"),
        false,
        t0(),
    );
    assert!(d.windows.is_empty() && m.count() == 0);
}

#[test]
fn download_burst_groups() {
    let mut m = Notifications::new();
    let s = Settings::default();
    let now = t0();
    let post = |m: &mut Notifications, f: &[&str], secs: i64| {
        m.post(
            &s,
            dl("Download ready", f),
            false,
            now + Duration::seconds(secs),
        );
    };
    post(&mut m, &["a"], 0);
    m.mark_displayed();
    let displayed = m.current().unwrap().clone();
    // The displayed snapshot is also the drag payload; queued arrivals cannot replace it.
    post(&mut m, &["b"], 1);
    post(&mut m, &["c"], 2);
    assert_eq!(m.count(), 2);
    assert_eq!(m.current(), Some(&displayed));
    assert_eq!(displayed.files.as_deref(), Some(&files(&["a"])[..]));
    m.tick(now + Duration::minutes(1), true, 12.0);
    assert_eq!(m.current(), Some(&displayed));
    assert!(m.dismiss());
    assert_eq!(m.count(), 1);
    assert_eq!(
        m.current().unwrap().files.as_deref(),
        Some(&files(&["b", "c"])[..])
    );
    m.mark_displayed();
    post(&mut m, &["d"], 3);
    m.dismiss();
    assert_eq!(
        m.current().unwrap().files.as_deref(),
        Some(&files(&["d"])[..])
    );
    m.clear();
    let forty: Vec<String> = (0..40).map(|i| format!("file-{i}")).collect();
    m.post(
        &s,
        InternalNotice::with_files(NoticeKind::Downloads, "Download ready", "inferred", forty),
        false,
        now,
    );
    post(&mut m, &["overflow"], 1);
    assert_eq!(m.count(), 2);
    assert_eq!(m.current().unwrap().files.as_ref().unwrap().len(), 40);
    m.dismiss();
    assert_eq!(
        m.current().unwrap().files.as_deref(),
        Some(&files(&["overflow"])[..])
    );
    assert!(!Notifications::new().dismiss());
}

#[test]
fn download_merge_is_case_insensitive() {
    let mut m = Notifications::new();
    let s = Settings::default();
    m.post(&s, dl("x", &["C:\\A.txt"]), false, t0());
    m.post(
        &s,
        dl("x", &["c:\\a.TXT", "C:\\b.txt"]),
        false,
        t0() + Duration::seconds(1),
    );
    assert_eq!(m.count(), 1);
    assert_eq!(m.current().unwrap().files.as_ref().unwrap().len(), 2);
}

#[test]
fn large_arrivals_keep_every_original() {
    for size in [41usize, 128] {
        let originals: Vec<String> = (0..size).map(|i| format!("arrival-{i:03}.txt")).collect();
        for mode in ["Notch", "Windows", "Both"] {
            let mut m = Notifications::new();
            let s = Settings {
                notification_delivery: mode.into(),
                ..Settings::default()
            };
            let incoming = InternalNotice::with_files(
                NoticeKind::Downloads,
                &format!("{size} downloads ready"),
                "Folder arrival inferred; drag originals or add to shelf",
                originals.clone(),
            );
            let d = m.post(&s, incoming, false, t0());
            let expected: Vec<&[String]> = originals.chunks(40).collect();
            assert_eq!(
                d.windows.len(),
                if mode == "Notch" { 0 } else { expected.len() }
            );
            for (notice, files) in d.windows.iter().zip(&expected) {
                assert_eq!(notice.files.as_deref(), Some(*files));
                assert_eq!(notice.title, downloads_title(files.len()));
            }
            assert_eq!(
                m.count(),
                if mode == "Windows" { 0 } else { expected.len() }
            );
            if mode == "Windows" {
                continue;
            }
            let mut accessible = Vec::new();
            for files in &expected {
                m.mark_displayed();
                let shown = m.current().unwrap().clone();
                assert_eq!(shown.files.as_deref(), Some(*files));
                assert_eq!(shown.title, downloads_title(files.len()));
                accessible.extend(shown.files.unwrap());
                m.dismiss();
            }
            assert_eq!(m.count(), 0);
            assert_eq!(accessible, originals);
        }
    }
}

#[test]
fn single_file_title_is_singular() {
    assert_eq!(downloads_title(1), "Download ready");
    assert_eq!(downloads_title(2), "2 downloads ready");
}
