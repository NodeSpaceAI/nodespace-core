//! Terminal output reduced to the text a reader would have seen.
//!
//! A PTY's output stream is text interleaved with escape sequences (colours,
//! cursor movement, window titles) and, for a full-screen agent, the same
//! screen repainted many times. [`strip_terminal_sequences`] removes the
//! sequences and the repaints, leaving lines of plain text. The result is
//! input for a summarizer, not a faithful replay: it keeps what was said, not
//! where on the screen it was drawn.

use std::collections::VecDeque;

/// How many of the most recent lines a new line is compared against. A
/// repainted screen re-emits lines already seen a few rows earlier; a line
/// that matches one of these is the repaint, not new output.
const REPAINT_WINDOW: usize = 64;

/// Where the scanner is in the output stream.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Ordinary text.
    Text,
    /// After `ESC`.
    Escape,
    /// After `ESC` and one or more intermediate bytes (`ESC ( B`).
    EscapeIntermediate,
    /// Inside a control sequence (`ESC [ … final`).
    Csi,
    /// Inside a string sequence (OSC, DCS, SOS, PM, APC), which runs to `BEL`
    /// or the string terminator.
    StringSequence,
    /// After `ESC` inside a string sequence: `\` completes the terminator.
    StringSequenceEscape,
}

/// Lines of plain text under construction.
#[derive(Default)]
struct Lines {
    done: Vec<String>,
    current: String,
    /// A carriage return was seen: the next printed character overwrites the
    /// line from its start. A line feed right after it is a plain line ending.
    carriage_returned: bool,
}

impl Lines {
    fn push(&mut self, c: char) {
        if self.carriage_returned {
            self.current.clear();
            self.carriage_returned = false;
        }
        self.current.push(c);
    }

    fn end_line(&mut self) {
        self.carriage_returned = false;
        self.done.push(std::mem::take(&mut self.current));
    }

    /// A gap the cursor jumped over, written as one space.
    fn gap(&mut self) {
        if !self.carriage_returned && !self.current.is_empty() && !self.current.ends_with(' ') {
            self.current.push(' ');
        }
    }
}

/// Remove every escape sequence and control character from terminal output,
/// and the repeated lines of a repainted screen.
///
/// The result holds only printable characters, spaces and `\n`. Lines with no
/// letter or digit (rules, box borders, spinners) are dropped, and so is a
/// line that repeats one of the lines just before it. A sequence the output
/// ends in the middle of is dropped with the rest.
pub fn strip_terminal_sequences(raw: &str) -> String {
    let mut lines = Lines::default();
    let mut state = State::Text;

    for c in raw.chars() {
        // `ESC` inside a string sequence is the only byte that needs a second
        // look: `ESC \` ends the string, and anything else starts a new
        // sequence.
        if state == State::StringSequenceEscape {
            if c == '\\' {
                state = State::Text;
                continue;
            }
            state = State::Escape;
        }

        state = match state {
            State::Text => text(c, &mut lines),
            State::Escape => match c {
                '[' => State::Csi,
                ']' | 'P' | 'X' | '^' | '_' => State::StringSequence,
                '\u{20}'..='\u{2f}' => State::EscapeIntermediate,
                '\u{1b}' => State::Escape,
                _ => State::Text,
            },
            State::EscapeIntermediate => match c {
                '\u{20}'..='\u{2f}' => State::EscapeIntermediate,
                '\u{1b}' => State::Escape,
                _ => State::Text,
            },
            State::Csi => match c {
                // Parameter and intermediate bytes.
                '\u{20}'..='\u{3f}' => State::Csi,
                '\u{1b}' => State::Escape,
                // The final byte. Cursor movement is where text stops being
                // contiguous, so it leaves a break; everything else (colours,
                // erasing, modes) leaves nothing.
                '\u{40}'..='\u{7e}' => {
                    match c {
                        'A' | 'B' | 'E' | 'F' | 'H' | 'd' | 'f' => {
                            if !lines.current.is_empty() {
                                lines.end_line();
                            }
                        }
                        'C' | 'G' => lines.gap(),
                        _ => {}
                    }
                    State::Text
                }
                '\n' => {
                    lines.end_line();
                    State::Csi
                }
                _ => State::Csi,
            },
            State::StringSequence => match c {
                // BEL and ST end the string; CAN and SUB abort it.
                '\u{07}' | '\u{9c}' | '\u{18}' | '\u{1a}' => State::Text,
                '\u{1b}' => State::StringSequenceEscape,
                _ => State::StringSequence,
            },
            State::StringSequenceEscape => unreachable!("resolved before the match"),
        };
    }
    // Whatever sequence the output ended inside is dropped; the text before
    // it is still a line.
    if !lines.current.is_empty() {
        lines.end_line();
    }

    without_repaints(lines.done)
}

/// One character of ordinary text.
fn text(c: char, lines: &mut Lines) -> State {
    match c {
        '\u{1b}' => return State::Escape,
        // The 8-bit forms of `ESC [` and of the string introducers.
        '\u{9b}' => return State::Csi,
        '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => return State::StringSequence,
        '\n' => lines.end_line(),
        '\r' => lines.carriage_returned = true,
        '\u{08}' => {
            lines.current.pop();
        }
        '\t' => lines.push(' '),
        // Every other control character, and the replacement character a
        // split multi-byte sequence decodes to.
        c if c.is_control() || c == char::REPLACEMENT_CHARACTER => {}
        c => lines.push(c),
    }
    State::Text
}

/// Join the lines worth keeping: those that say something, and that are not a
/// repaint of a line just before them.
fn without_repaints(lines: Vec<String>) -> String {
    let mut recent: VecDeque<String> = VecDeque::with_capacity(REPAINT_WINDOW);
    let mut out = String::new();
    for line in lines {
        let line = line.trim();
        if !line.chars().any(char::is_alphanumeric) || recent.iter().any(|seen| seen == line) {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        if recent.len() == REPAINT_WINDOW {
            recent.pop_front();
        }
        recent.push_back(line.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing a terminal would interpret survives: only printable
    /// characters, spaces and line feeds.
    fn assert_plain(text: &str) {
        for c in text.chars() {
            assert!(
                c == '\n' || !c.is_control(),
                "control character {:?} in {text:?}",
                c
            );
        }
        assert!(!text.contains(char::REPLACEMENT_CHARACTER), "{text:?}");
    }

    #[test]
    fn colours_and_styles_are_removed() {
        let out = strip_terminal_sequences("\x1b[1;32mBuild passed\x1b[0m in \x1b[38;5;208m3s\x1b[m");
        assert_eq!(out, "Build passed in 3s");
    }

    #[test]
    fn window_titles_and_hyperlinks_are_removed_whichever_way_they_end() {
        let out = strip_terminal_sequences(
            "\x1b]0;claude — secret/path\x07Ready\n\
             \x1b]8;;https://example.com\x1b\\the docs\x1b]8;;\x1b\\ are here",
        );
        assert_eq!(out, "Ready\nthe docs are here");
    }

    #[test]
    fn device_control_and_application_strings_are_removed() {
        let out = strip_terminal_sequences("a\x1bPq#0;2;0;0;0\x1b\\b\x1b_payload\x1b\\c\x1b^pm\x1b\\d");
        assert_eq!(out, "abcd");
    }

    #[test]
    fn charset_and_keypad_escapes_are_removed() {
        let out = strip_terminal_sequences("\x1b(B\x1b=\x1b7plain\x1b8\x1b>");
        assert_eq!(out, "plain");
    }

    #[test]
    fn eight_bit_sequences_are_removed() {
        let out = strip_terminal_sequences("\u{9b}31mred\u{9b}0m \u{9d}0;title\u{9c}text");
        assert_eq!(out, "red text");
    }

    #[test]
    fn control_characters_are_removed_and_tabs_become_spaces() {
        let out = strip_terminal_sequences("one\ttwo\x07\x00\x7f three\x0b");
        assert_eq!(out, "one two three");
        assert_plain(&out);
    }

    #[test]
    fn a_carriage_return_overwrites_the_line_and_crlf_ends_it() {
        let out = strip_terminal_sequences("Downloading 10%\rDownloading 100%\r\nDone\r\n");
        assert_eq!(out, "Downloading 100%\nDone");
    }

    #[test]
    fn a_backspace_removes_the_character_before_it() {
        assert_eq!(strip_terminal_sequences("lss\x08 -la"), "ls -la");
    }

    #[test]
    fn cursor_movement_separates_text_instead_of_gluing_it() {
        // A full-screen agent positions text with the cursor, not with
        // spaces and line feeds.
        let out = strip_terminal_sequences("\x1b[2J\x1b[1;1HEdit file\x1b[3;1HRun tests\x1b[5Cnow");
        assert_eq!(out, "Edit file\nRun tests now");
    }

    #[test]
    fn a_repainted_screen_is_kept_once() {
        let frame = "\x1b[2K\x1b[1A\x1b[2K\x1b[G╭──────────╮\r\n│ Thinking │\r\n╰──────────╯\r\n";
        let out = strip_terminal_sequences(&format!("{frame}{frame}{frame}Fixed the parser\r\n"));
        assert_eq!(out, "│ Thinking │\nFixed the parser");
    }

    #[test]
    fn lines_with_nothing_to_read_are_dropped() {
        let out = strip_terminal_sequences("first\n\n   \n────────\n⠋\nsecond\n");
        assert_eq!(out, "first\nsecond");
    }

    #[test]
    fn output_that_ends_inside_a_sequence_drops_the_sequence() {
        assert_eq!(strip_terminal_sequences("done\x1b[38;5"), "done");
        assert_eq!(strip_terminal_sequences("done\x1b]0;half a title"), "done");
        assert_eq!(strip_terminal_sequences("done\x1b"), "done");
    }

    #[test]
    fn an_escape_inside_a_string_sequence_starts_a_new_sequence() {
        // The title is never terminated; the colour sequence that follows
        // still ends, and the text after it is kept.
        let out = strip_terminal_sequences("\x1b]0;title\x1b[31mred\x1b[0m");
        assert_eq!(out, "red");
    }

    #[test]
    fn plain_text_passes_through() {
        let text = "Refactored the parser.\nAll 42 tests pass — naïve café ✓";
        assert_eq!(strip_terminal_sequences(text), text);
    }
}
