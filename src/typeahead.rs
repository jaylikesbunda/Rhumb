//! Type-ahead find, the way Explorer jumps to an item as you type.
//!
//! Kept separate from the UI so the matching rules can be tested: a prefix
//! match on the name, restarted after a pause, and wrapped around the list.

/// How long a pause clears the buffer and starts a new search.
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(900);

#[derive(Default)]
pub struct TypeAhead {
    /// What has been typed so far, lowercased for comparison.
    buffer: String,
    typed_at: Option<std::time::Instant>,
}

impl TypeAhead {
    /// Feeds a character in, returning the buffer to search for.
    ///
    /// Returns an empty string when the pause elapsed, because that starts a
    /// fresh search rather than extending the old one.
    pub fn push(&mut self, ch: char, now: std::time::Instant) -> &str {
        if self
            .typed_at
            .is_some_and(|t| now.duration_since(t) > TIMEOUT)
        {
            self.buffer.clear();
        }
        // A space starts a new word rather than extending the last one.
        if ch == ' ' {
            self.buffer.clear();
        } else {
            self.buffer.push(ch.to_ascii_lowercase());
        }
        self.typed_at = Some(now);
        &self.buffer
    }

    /// Forgets the buffer, e.g. after a navigation or a click.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.typed_at = None;
    }

    /// The row to jump to, searching forward from `from` and wrapping around.
    ///
    /// `names` is indexed by row. Returns `None` when nothing starts with the
    /// buffer, leaving the cursor where it was.
    pub fn find(&self, names: &[String], from: usize) -> Option<usize> {
        if self.buffer.is_empty() || names.is_empty() {
            return None;
        }
        let needle = &self.buffer;
        (0..names.len())
            .map(|i| (from + 1 + i) % names.len())
            .find(|&i| names[i].to_lowercase().starts_with(needle))
    }
}

/// The character a key press contributes to the buffer, if any.
///
/// egui reports keys rather than text, so the letters and digits are mapped
/// explicitly. Anything else - arrows, function keys, punctuation - returns
/// `None` and never reaches the buffer.
pub fn typed_char(key: egui::Key) -> Option<char> {
    use egui::Key as K;
    Some(match key {
        K::Num0 => '0',
        K::Num1 => '1',
        K::Num2 => '2',
        K::Num3 => '3',
        K::Num4 => '4',
        K::Num5 => '5',
        K::Num6 => '6',
        K::Num7 => '7',
        K::Num8 => '8',
        K::Num9 => '9',
        K::A => 'a',
        K::B => 'b',
        K::C => 'c',
        K::D => 'd',
        K::E => 'e',
        K::F => 'f',
        K::G => 'g',
        K::H => 'h',
        K::I => 'i',
        K::J => 'j',
        K::K => 'k',
        K::L => 'l',
        K::M => 'm',
        K::N => 'n',
        K::O => 'o',
        K::P => 'p',
        K::Q => 'q',
        K::R => 'r',
        K::S => 's',
        K::T => 't',
        K::U => 'u',
        K::V => 'v',
        K::W => 'w',
        K::X => 'x',
        K::Y => 'y',
        K::Z => 'z',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn names() -> Vec<String> {
        ["Alpha", "beta", "Gamma", "alphabet"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn finds_the_next_prefix_match_and_wraps() {
        let mut t = TypeAhead::default();
        t.push('g', Instant::now());
        // Gamma is the only match, so any search lands on it, wrapping included.
        assert_eq!(t.find(&names(), 0), Some(2));
        assert_eq!(t.find(&names(), 1), Some(2));
        assert_eq!(t.find(&names(), 2), Some(2));
        assert_eq!(t.find(&names(), 3), Some(2));
    }

    #[test]
    fn repeated_letters_walk_past_the_current_row() {
        let mut t = TypeAhead::default();
        t.push('a', Instant::now());
        // Both Alpha and alphabet start with "a", so pressing "a" again steps on.
        assert_eq!(t.find(&names(), 0), Some(3));
        assert_eq!(t.find(&names(), 3), Some(0));
    }

    #[test]
    fn matching_ignores_case() {
        let mut t = TypeAhead::default();
        t.push('a', Instant::now());
        assert_eq!(t.find(&names(), 3), Some(0));
    }

    #[test]
    fn a_pause_starts_a_new_search() {
        let mut t = TypeAhead::default();
        let now = Instant::now();
        t.push('a', now);
        t.push('l', now);
        assert_eq!(t.buffer, "al");
        // Past the timeout the buffer restarts with just this character.
        t.push('g', now + TIMEOUT + Duration::from_millis(1));
        assert_eq!(t.buffer, "g");
    }

    #[test]
    fn a_space_starts_a_new_word() {
        let mut t = TypeAhead::default();
        let now = Instant::now();
        t.push('a', now);
        t.push(' ', now);
        assert_eq!(t.buffer, "");
        t.push('g', now);
        assert_eq!(t.buffer, "g");
    }

    #[test]
    fn no_match_leaves_the_cursor_alone() {
        let mut t = TypeAhead::default();
        t.push('z', Instant::now());
        assert_eq!(t.find(&names(), 0), None);
        t.clear();
        // Clearing means the next character starts a fresh search.
        t.push('g', Instant::now());
        assert_eq!(t.find(&names(), 0), Some(2));
    }

    #[test]
    fn an_empty_list_never_matches() {
        let mut t = TypeAhead::default();
        t.push('a', Instant::now());
        assert_eq!(t.find(&[], 0), None);
    }

    #[test]
    fn only_letters_and_digits_type() {
        assert_eq!(typed_char(egui::Key::A), Some('a'));
        assert_eq!(typed_char(egui::Key::Num7), Some('7'));
        // Arrows, function keys and punctuation must not reach the buffer.
        for key in [
            egui::Key::ArrowDown,
            egui::Key::Enter,
            egui::Key::Escape,
            egui::Key::F5,
            egui::Key::Slash,
            egui::Key::Space,
        ] {
            assert_eq!(typed_char(key), None, "{key:?} should not type");
        }
    }
}
