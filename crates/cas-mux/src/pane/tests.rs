#[cfg(test)]
mod cases {
    use crate::pane::{Pane, PaneKind};

    #[test]
    fn test_pane_kind_as_str() {
        assert_eq!(PaneKind::Worker.as_str(), "worker");
        assert_eq!(PaneKind::Supervisor.as_str(), "supervisor");
        assert_eq!(PaneKind::Director.as_str(), "director");
    }

    #[test]
    fn test_director_pane() {
        let pane = Pane::director("test-director", 24, 80).expect("create pane");
        assert_eq!(pane.id(), "test-director");
        assert_eq!(pane.kind(), &PaneKind::Director);
        assert_eq!(pane.title(), "Director");
        assert!(!pane.has_exited());
    }

    #[test]
    fn test_pane_feed_and_dump() {
        let mut pane = Pane::director("test", 24, 80).expect("create pane");
        pane.feed(b"Hello, World!").expect("feed");
        let content = pane.dump_viewport().expect("dump viewport");
        assert!(content.contains("Hello, World!"));
    }

    #[test]
    fn test_pane_ansi_colors() {
        let mut pane = Pane::director("test", 24, 80).expect("create pane");
        pane.feed(b"\x1b[31mRed\x1b[0m Normal").expect("feed");
        let content = pane.dump_viewport().expect("dump viewport");
        assert!(content.contains("Red"));
        assert!(content.contains("Normal"));
    }

    #[test]
    fn test_pane_scroll() {
        let mut pane = Pane::director("test", 5, 80).expect("create pane");

        for i in 0..20 {
            pane.feed(format!("Line {i}\r\n").as_bytes())
                .expect("feed line");
        }

        assert!(pane.scroll(-5).is_ok());
        assert!(pane.scroll(3).is_ok());
        assert!(pane.scroll_to_top().is_ok());
        assert!(pane.scroll_to_bottom().is_ok());
    }

    #[test]
    fn test_scrollback_info() {
        let pane = Pane::director("test", 24, 80).expect("create pane");
        let info = pane.scrollback_info();

        assert_eq!(info.viewport_offset, 0);
        assert_eq!(info.viewport_rows, 24);
        assert!(info.total_scrollback >= info.viewport_rows as u32);
    }

    #[test]
    fn test_get_full_snapshot() {
        let mut pane = Pane::director("test", 5, 10).expect("create pane");
        for i in 0..5 {
            pane.feed(format!("Line {i}\r\n").as_bytes())
                .expect("feed line");
        }

        let snapshot = pane.get_full_snapshot().expect("snapshot");
        assert_eq!(snapshot.rows, 5);
        assert_eq!(snapshot.cols, 10);
        assert_eq!(snapshot.cells.len(), 50);
    }

    #[test]
    fn test_create_snapshot_no_cache() {
        let mut pane = Pane::director("test", 5, 10).expect("create pane");
        for i in 0..5 {
            pane.feed(format!("Row {i}\r\n").as_bytes())
                .expect("feed row");
        }

        let (snapshot, cache_rows, cache_start) = pane
            .create_snapshot_with_cache(0)
            .expect("snapshot with cache");
        assert_eq!(snapshot.rows, 5);
        assert!(cache_rows.is_empty());
        assert!(cache_start.is_none());
    }

    #[test]
    fn test_create_snapshot_with_cache() {
        let mut pane = Pane::director("test", 5, 80).expect("create pane");

        for i in 0..30 {
            pane.feed(format!("Line {i}\r\n").as_bytes())
                .expect("feed line");
        }

        pane.scroll(-10).expect("scroll up");

        let (snapshot, cache_rows, cache_start) = pane
            .create_snapshot_with_cache(20)
            .expect("snapshot with cache");
        assert_eq!(snapshot.rows, 5);
        assert!(cache_start.is_some());

        for row in &cache_rows {
            assert!(row.screen_row < pane.scrollback_lines());
        }
    }

    #[test]
    fn test_strip_literal_cursor_report_echo() {
        let input = b"hello ^[[12;34R world";
        let cleaned = Pane::strip_literal_cursor_reports(input);
        assert_eq!(cleaned.as_ref(), b"hello  world");
    }

    #[test]
    fn test_strip_literal_cursor_report_noop_for_normal_text() {
        let input = b"normal output with [brackets] and numbers 12;34R";
        let cleaned = Pane::strip_literal_cursor_reports(input);
        assert_eq!(cleaned.as_ref(), input);
    }

    // =========================================================================
    // update_alt_screen unit tests — verify the fixed outer-loop guard
    // (was `while i + 4 < data.len()`, now `while i < data.len()` with inner
    //  bounds checks). These test sequences short enough that the old guard
    //  would silently skip them.
    // =========================================================================

    #[test]
    fn update_alt_screen_handles_minimum_length_seq() {
        // Shortest valid sequence: ESC [ ? 4 7 h = 6 bytes (mode 47).
        // With the old guard (i + 4 < len) this was only reached when i == 0
        // AND len > 4, but if the data was exactly 6 bytes the loop ran while
        // 0 + 4 < 6, i.e. for i in 0..1. That still works — but let's confirm.
        let data = b"\x1b[?47h";
        assert!(Pane::update_alt_screen(data, false));
    }

    #[test]
    fn update_alt_screen_detects_1049_entry() {
        let data = b"\x1b[?1049h";
        assert!(Pane::update_alt_screen(data, false));
    }

    #[test]
    fn update_alt_screen_last_sequence_wins() {
        // Enter then exit in the same slice — last (exit) must win.
        let data = b"\x1b[?1049h\x1b[?1049l";
        assert!(!Pane::update_alt_screen(data, false));
    }

    #[test]
    fn update_alt_screen_preserves_current_on_empty_input() {
        assert!(Pane::update_alt_screen(b"", true));
        assert!(!Pane::update_alt_screen(b"", false));
    }

    // =========================================================================
    // trailing_dec_partial unit tests — verify carry-buffer detection
    // =========================================================================

    #[test]
    fn trailing_dec_partial_bare_esc() {
        let data = b"hello\x1b";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b");
    }

    #[test]
    fn trailing_dec_partial_esc_bracket() {
        let data = b"abc\x1b[";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b[");
    }

    #[test]
    fn trailing_dec_partial_esc_bracket_question() {
        let data = b"\x1b[?";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b[?");
    }

    #[test]
    fn trailing_dec_partial_esc_bracket_question_digits() {
        let data = b"junk\x1b[?104";
        let partial = Pane::trailing_dec_partial(data);
        assert_eq!(partial, b"\x1b[?104");
    }

    #[test]
    fn trailing_dec_partial_complete_sequence_not_partial() {
        // A complete sequence should NOT be kept — it ends with h/l.
        let data = b"\x1b[?1049h";
        let partial = Pane::trailing_dec_partial(data);
        assert!(partial.is_empty(), "complete sequence must not be carried: {partial:?}");
    }

    #[test]
    fn trailing_dec_partial_empty_input() {
        assert!(Pane::trailing_dec_partial(b"").is_empty());
    }

    #[test]
    fn trailing_dec_partial_no_esc() {
        assert!(Pane::trailing_dec_partial(b"hello world").is_empty());
    }
}
