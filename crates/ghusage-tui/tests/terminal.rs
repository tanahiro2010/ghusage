//! Terminal restoration on exit and on panic.

use ghusage_tui::term;

#[test]
fn restore_sequence_leaves_alternate_screen_and_shows_cursor() {
    let mut out = Vec::new();
    term::write_restore_sequence(&mut out).unwrap();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("\u{1b}[?1049l"), "{s:?}");
    assert!(s.contains("\u{1b}[?25h"), "{s:?}");
}

#[test]
fn restoring_a_terminal_that_was_never_switched_is_harmless() {
    drop(term::restore_terminal());
    drop(term::restore_terminal());
}

#[test]
fn panic_hook_restores_the_terminal_and_quiet_threads_are_left_alone() {
    term::install_panic_hook();
    term::install_panic_hook(); // installing twice is a no-op
    let before = term::panic_restore_count();
    let r = std::panic::catch_unwind(|| panic!("boom"));
    assert!(r.is_err());
    assert_eq!(term::panic_restore_count(), before + 1);

    // A panic in a background worker does not tear down the UI.
    let h = std::thread::spawn(|| {
        term::set_quiet_panics(true);
        drop(std::panic::catch_unwind(|| panic!("worker")));
    });
    h.join().unwrap();
    assert_eq!(term::panic_restore_count(), before + 1);
}

#[test]
fn only_github_https_urls_can_be_opened() {
    assert!(term::checked_url("https://github.com/o/r/pull/1").is_ok());
    assert!(term::checked_url("http://github.com/o/r").is_err());
    assert!(term::checked_url("https://evil.example/o/r").is_err());
    assert!(term::checked_url("https://github.com@evil.example/").is_err());
    assert!(term::checked_url("file:///etc/passwd").is_err());
    assert!(term::checked_url("https://github.com/o/r\u{1b}[2J").is_err());
}
