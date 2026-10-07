//! The text of a document, held as a rope.
//!
//! A `String` keeps every character in one run of bytes, so an edit in the middle
//! shifts everything after it, and turning a character index into a byte offset
//! means walking from the top. A rope keeps the text in a balanced tree of small
//! chunks: an edit, a character-to-line lookup and a character count are all
//! O(log n), and a copy of the whole thing is O(1), which is what makes holding a
//! multi-megabyte file and typing into it cost the same as a small one.
//!
//! Every position in this module is a *character* index, because that is what the
//! caret, the selection and the layout speak in. Nothing here is a byte offset
//! except the two functions that say so.
//!
//! Only `\n` ends a line. The rope is built without the unicode and carriage-return
//! line-break features on purpose: the editor's notion of a line has always been
//! "up to the next `\n`", and a stray `\r` or U+2028 splitting a line would move
//! every line number after it.
//!
//! Every edit is also written to a journal. The editor drains it when an undo step
//! closes, so a step is the list of edits it made and undoing is replaying them
//! backwards - no snapshot of the file, and no diff to work out afterwards.

use ropey::{Rope, RopeBuilder};
use std::borrow::Cow;
use std::fmt;

/// One change made to the buffer, kept so it can be taken back.
///
/// `removed` was at `at` and `inserted` stands there now.
#[derive(Clone, Debug)]
pub struct Edit {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
    removed_chars: usize,
    inserted_chars: usize,
}

/// A document's text.
#[derive(Clone, Default)]
pub struct Buffer {
    rope: Rope,
    journal: Vec<Edit>,
    /// While on, the position and size of every change, so a caller that runs several
    /// edits can follow what they did to the places it is holding.
    trace: Option<Vec<(usize, usize, usize)>>,
    /// The first line any edit has touched since somebody last asked.
    dirty_from: Option<usize>,
}

impl Buffer {
    pub fn new() -> Buffer {
        Buffer::default()
    }

    // ---- size ---------------------------------------------------------------

    /// How many characters there are. O(1).
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// How many bytes the text would take as UTF-8. O(1).
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    // ---- lines --------------------------------------------------------------

    /// How many lines there are: one more than the newlines, so an empty
    /// document has one and `"a\n"` has two.
    pub fn lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// The character index where `line` starts. Lines past the end clamp to the
    /// last.
    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.last_line()))
    }

    /// The character index where `line`'s content ends, not counting its newline.
    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.last_line());
        if line == self.last_line() {
            self.rope.len_chars()
        } else {
            self.rope.line_to_char(line + 1) - 1
        }
    }

    /// How many characters `line` holds, not counting its newline.
    pub fn line_len(&self, line: usize) -> usize {
        self.line_end(line) - self.line_start(line)
    }

    /// Which line a character index is on. An index past the end is on the last.
    pub fn line_of_char(&self, at: usize) -> usize {
        self.rope.char_to_line(at.min(self.rope.len_chars()))
    }

    /// The text of `line` without its newline. Lines past the end are empty.
    ///
    /// Borrowed when the line sits inside one chunk of the tree, which is nearly
    /// always, and copied when it straddles two.
    pub fn line_str(&self, line: usize) -> Cow<'_, str> {
        if line > self.last_line() {
            return Cow::Borrowed("");
        }
        let (a, b) = (self.line_start(line), self.line_end(line));
        cow(self.rope.slice(a..b))
    }

    /// The characters in `lo..hi`, clamped to the document.
    pub fn slice(&self, lo: usize, hi: usize) -> Cow<'_, str> {
        let n = self.rope.len_chars();
        let hi = hi.min(n);
        let lo = lo.min(hi);
        cow(self.rope.slice(lo..hi))
    }

    fn last_line(&self) -> usize {
        self.rope.len_lines().saturating_sub(1)
    }

    // ---- characters ---------------------------------------------------------

    /// Whether the last character is a newline.
    pub fn ends_with_newline(&self) -> bool {
        self.rope.len_chars() > 0 && self.rope.char(self.rope.len_chars() - 1) == '\n'
    }

    /// The character at `at`, if there is one.
    pub fn char_at(&self, at: usize) -> Option<char> {
        (at < self.rope.len_chars()).then(|| self.rope.char(at))
    }

    /// Every character, in order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn chars(&self) -> ropey::iter::Chars<'_> {
        self.rope.chars()
    }

    /// Every character from `at` on, without reading the ones before it.
    pub fn chars_at(&self, at: usize) -> ropey::iter::Chars<'_> {
        self.rope.chars_at(at.min(self.rope.len_chars()))
    }

    /// The chunks of text the rope is made of, in order. Each is valid UTF-8.
    pub fn chunks(&self) -> impl Iterator<Item = &str> {
        self.rope.chunks()
    }

    /// The byte offset of a character index.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn byte_of_char(&self, at: usize) -> usize {
        self.rope.char_to_byte(at.min(self.rope.len_chars()))
    }

    /// The character index of a byte offset.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn char_of_byte(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte.min(self.rope.len_bytes()))
    }

    // ---- editing ------------------------------------------------------------

    /// Puts `s` in at `at`.
    pub fn insert(&mut self, at: usize, s: &str) {
        self.replace(at, at, s);
    }

    /// Takes `lo..hi` out.
    pub fn remove(&mut self, lo: usize, hi: usize) {
        self.replace(lo, hi, "");
    }

    /// Replaces `lo..hi` with `s`, and records it.
    ///
    /// The range is clamped to the document, so a position that is a character out
    /// of date costs a wrong edit rather than a panic.
    pub fn replace(&mut self, lo: usize, hi: usize, s: &str) {
        let n = self.rope.len_chars();
        let hi = hi.min(n);
        let lo = lo.min(hi);
        // Line endings are `\n` here and nothing else, whatever the text came from:
        // the Windows clipboard hands over CRLF, and a `\r` that got in would be a
        // character the caret could stand on the wrong side of.
        let unified;
        let s = if s.contains('\r') {
            unified = s.replace("\r\n", "\n").replace('\r', "\n");
            unified.as_str()
        } else {
            s
        };
        if lo == hi && s.is_empty() {
            return;
        }
        let removed = self.rope.slice(lo..hi).to_string();
        if let Some(t) = &mut self.trace {
            t.push((lo, hi - lo, s.chars().count()));
        }
        let edit = Edit {
            at: lo,
            removed_chars: hi - lo,
            inserted_chars: s.chars().count(),
            removed,
            inserted: s.to_owned(),
        };
        self.splice(lo, hi, s);
        self.journal.push(edit);
    }

    /// Starts noting the position and size of every change, from now.
    pub fn trace_begin(&mut self) {
        self.trace = Some(Vec::new());
    }

    /// Stops noting changes, and returns them as `(at, removed, inserted)` in the
    /// order they were made, with the sizes in characters.
    pub fn trace_take(&mut self) -> Vec<(usize, usize, usize)> {
        self.trace.take().unwrap_or_default()
    }

    /// The character index of the first place at or after `from` where `needle`
    /// stands, which may run across the chunks the rope is held in.
    pub fn find_from(&self, needle: &str, from: usize) -> Option<usize> {
        if needle.is_empty() {
            return None;
        }
        let start = self.byte_of_char(from.min(self.len_chars()));
        let (chunks, chunk_start, _, _) = self.rope.chunks_at_byte(start);
        let mut carry = String::new();
        let mut base = start;
        for (n, chunk) in chunks.enumerate() {
            let piece = if n == 0 {
                &chunk[start - chunk_start..]
            } else {
                chunk
            };
            let hay: Cow<str> = if carry.is_empty() {
                Cow::Borrowed(piece)
            } else {
                Cow::Owned(format!("{carry}{piece}"))
            };
            if let Some(pos) = hay.find(needle) {
                return Some(self.char_of_byte(base + pos));
            }
            // The tail that a match could still begin in goes on to the next chunk.
            let keep = needle.len().saturating_sub(1).min(hay.len());
            let mut cut = hay.len() - keep;
            while !hay.is_char_boundary(cut) {
                cut += 1;
            }
            carry = hay[cut..].to_owned();
            base += cut;
        }
        None
    }

    /// Takes the journal, leaving it empty.
    pub fn take_journal(&mut self) -> Vec<Edit> {
        std::mem::take(&mut self.journal)
    }

    /// Whether there are edits nobody has taken yet.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_journal(&self) -> bool {
        !self.journal.is_empty()
    }

    /// Whether `edit`'s inserted text is what stands at its position now.
    pub fn holds_inserted(&self, edit: &Edit) -> bool {
        self.matches_at(edit.at, &edit.inserted, edit.inserted_chars)
    }

    /// Whether `edit`'s removed text is what stands at its position now.
    pub fn holds_removed(&self, edit: &Edit) -> bool {
        self.matches_at(edit.at, &edit.removed, edit.removed_chars)
    }

    fn matches_at(&self, at: usize, s: &str, chars: usize) -> bool {
        at + chars <= self.rope.len_chars() && self.rope.slice(at..at + chars) == s
    }

    /// Takes `edit` back, without recording it.
    pub fn undo_edit(&mut self, edit: &Edit) {
        self.splice(edit.at, edit.at + edit.inserted_chars, &edit.removed);
    }

    /// Does `edit` again, without recording it.
    pub fn redo_edit(&mut self, edit: &Edit) {
        self.splice(edit.at, edit.at + edit.removed_chars, &edit.inserted);
    }

    fn splice(&mut self, lo: usize, hi: usize, s: &str) {
        // Everything from the first line touched down is now something else, which
        // is all a reader of the text above it needs to know to stay correct.
        let line = self.rope.char_to_line(lo.min(self.rope.len_chars()));
        self.dirty_from = Some(self.dirty_from.map_or(line, |d| d.min(line)));
        if hi > lo {
            self.rope.remove(lo..hi);
        }
        if !s.is_empty() {
            self.rope.insert(lo, s);
        }
    }

    /// Reads a whole stream into a buffer a block at a time, never holding more
    /// than one block of it outside the rope.
    ///
    /// Bytes that are not UTF-8 become U+FFFD, one for each bad stretch, which is
    /// what `String::from_utf8_lossy` does, so a file loads the same way whichever
    /// route it takes. A character split across two blocks is carried over and
    /// decoded whole rather than mistaken for damage.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_reader<R: std::io::Read>(reader: R) -> std::io::Result<Buffer> {
        let mut builder = RopeBuilder::new();
        decode_lossy(reader, &mut |s| builder.append(s))?;
        Ok(Buffer {
            rope: builder.finish(),
            journal: Vec::new(),
            dirty_from: None,
            trace: None,
        })
    }

    /// [`Buffer::from_reader`], with Windows line endings turned into `\n`.
    ///
    /// Returns whether any were found, so the caller can put them back on save.
    /// The editor works in `\n` only: a `\r` left in the text is a character the
    /// caret can stand in front of and behind, so a line would have an invisible
    /// position at its end, End and a click past the last letter would land on the
    /// wrong side of it, and typing there would go in between the CR and the LF.
    pub fn from_reader_lf<R: std::io::Read>(reader: R) -> std::io::Result<(Buffer, bool)> {
        let mut builder = RopeBuilder::new();
        let mut saw_crlf = false;
        // A CR that ended one piece of text, which is a line ending only if the next
        // piece starts with the LF.
        let mut held_cr = false;
        decode_lossy(reader, &mut |s| {
            let mut s = s;
            if held_cr {
                held_cr = false;
                if s.starts_with('\n') {
                    saw_crlf = true;
                } else {
                    builder.append("\r");
                }
            }
            if let Some(rest) = s.strip_suffix('\r') {
                held_cr = true;
                s = rest;
            }
            if s.contains("\r\n") {
                saw_crlf = true;
                builder.append(&s.replace("\r\n", "\n"));
            } else {
                builder.append(s);
            }
        })?;
        if held_cr {
            builder.append("\r");
        }
        Ok((
            Buffer {
                rope: builder.finish(),
                journal: Vec::new(),
                dirty_from: None,
                trace: None,
            },
            saw_crlf,
        ))
    }

    /// The first line edited since the last call, and forgets it.
    pub fn take_dirty_line(&mut self) -> Option<usize> {
        self.dirty_from.take()
    }

    /// A copy of the text that can be read on another thread while this one goes on
    /// being edited. The rope shares its nodes, so this is a pointer bump and not a
    /// copy of the document; the journal is left behind.
    pub fn snapshot(&self) -> Buffer {
        Buffer {
            rope: self.rope.clone(),
            journal: Vec::new(),
            dirty_from: None,
            trace: None,
        }
    }

    /// The text as a `String`. O(n), for the few places that need it all at once.
    pub fn to_text(&self) -> String {
        self.rope.to_string()
    }
}

/// Reads a stream a block at a time and hands the decoded text to `sink` in
/// pieces, never holding more than one block.
///
/// Bytes that are not UTF-8 become U+FFFD, one for each bad stretch, which is what
/// `String::from_utf8_lossy` does. A character split across two blocks is carried
/// over and decoded whole rather than mistaken for damage.
fn decode_lossy<R: std::io::Read>(
    mut reader: R,
    sink: &mut dyn FnMut(&str),
) -> std::io::Result<()> {
    let mut block = vec![0u8; 1 << 16];
    // The start of a character that was cut off by the end of the last block.
    let mut carry: Vec<u8> = Vec::new();
    loop {
        let n = match reader.read(&mut block) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        carry.extend_from_slice(&block[..n]);
        let mut at = 0;
        while at < carry.len() {
            match std::str::from_utf8(&carry[at..]) {
                Ok(good) => {
                    sink(good);
                    at = carry.len();
                }
                Err(e) => {
                    let ok = e.valid_up_to();
                    if let Ok(good) = std::str::from_utf8(&carry[at..at + ok]) {
                        sink(good);
                    }
                    at += ok;
                    match e.error_len() {
                        Some(bad) => {
                            sink("\u{FFFD}");
                            at += bad;
                        }
                        // Ran off the end mid-character: keep it for the next block.
                        None => break,
                    }
                }
            }
        }
        carry.drain(..at);
    }
    if !carry.is_empty() {
        sink("\u{FFFD}");
    }
    Ok(())
}

/// A rope slice as text: borrowed if it is one chunk, copied if not.
fn cow(slice: ropey::RopeSlice<'_>) -> Cow<'_, str> {
    match slice.as_str() {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(slice.to_string()),
    }
}

impl From<&str> for Buffer {
    fn from(s: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(s),
            journal: Vec::new(),
            dirty_from: None,
            trace: None,
        }
    }
}

impl From<String> for Buffer {
    fn from(s: String) -> Buffer {
        Buffer::from(s.as_str())
    }
}

impl fmt::Display for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.rope.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.to_text())
    }
}

impl PartialEq<str> for Buffer {
    fn eq(&self, other: &str) -> bool {
        self.rope == other
    }
}

impl PartialEq<&str> for Buffer {
    fn eq(&self, other: &&str) -> bool {
        self.rope == *other
    }
}

impl PartialEq<String> for Buffer {
    fn eq(&self, other: &String) -> bool {
        self.rope == other.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_count_newlines_and_nothing_else() {
        assert_eq!(Buffer::from("").lines(), 1);
        assert_eq!(Buffer::from("a").lines(), 1);
        assert_eq!(Buffer::from("a\n").lines(), 2);
        assert_eq!(Buffer::from("a\nb\n\nc").lines(), 4);
        // Carriage returns and the unicode separators are not line breaks here.
        assert_eq!(Buffer::from("a\rb\u{2028}c\r\nd").lines(), 2);
    }

    #[test]
    fn a_line_is_its_content_without_the_newline() {
        let b = Buffer::from("héllo\nwörld\n\nx");
        assert_eq!(b.line_str(0), "héllo");
        assert_eq!(b.line_str(1), "wörld");
        assert_eq!(b.line_str(2), "");
        assert_eq!(b.line_str(3), "x");
        assert_eq!(b.line_str(9), "", "past the end is empty");
        assert_eq!((b.line_start(1), b.line_end(1)), (6, 11));
        assert_eq!(b.line_len(0), 5);
        assert_eq!(b.line_of_char(6), 1);
        assert_eq!(b.line_of_char(999), 3, "past the end is the last line");
    }

    #[test]
    fn positions_are_characters_and_clamp() {
        let mut b = Buffer::from("aéb");
        assert_eq!((b.len_chars(), b.len_bytes()), (3, 4));
        assert_eq!(b.byte_of_char(2), 3);
        assert_eq!(b.char_of_byte(3), 2);
        b.replace(1, 99, "Z");
        assert_eq!(b, "aZ", "a range past the end is cut back");
        b.insert(99, "!");
        assert_eq!(b, "aZ!", "an insert past the end lands at the end");
    }

    #[test]
    fn random_edits_on_a_large_buffer_match_a_string_and_undo_completely() {
        // Large enough that the rope is many chunks deep, with multi-byte
        // characters and newlines, so an off-by-one at a chunk boundary or in a
        // character-to-byte conversion has somewhere to show.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        let alphabet = ["a", "é", "\n", "日", "b", " ", "😀"];
        let make = |next: &mut dyn FnMut(usize) -> usize, n: usize| -> String {
            (0..n).map(|_| alphabet[next(alphabet.len())]).collect()
        };
        let start = make(&mut next, 300_000);
        let mut model: Vec<char> = start.chars().collect();
        let mut buf = Buffer::from(start.as_str());
        for _ in 0..400 {
            let len = model.len();
            let lo = next(len + 1);
            let hi = (lo + next(40)).min(len);
            let n = next(30);
            let with = make(&mut next, n);
            buf.replace(lo, hi, &with);
            model.splice(lo..hi, with.chars());
        }
        let want: String = model.iter().collect();
        assert_eq!(buf.len_chars(), model.len());
        assert_eq!(buf.len_bytes(), want.len());
        assert_eq!(buf.to_text(), want);
        assert_eq!(buf.lines(), want.matches('\n').count() + 1);
        // Line lookups agree with counting newlines by hand, at scattered points.
        for _ in 0..200 {
            let at = next(model.len() + 1);
            let line = model[..at].iter().filter(|&&c| c == '\n').count();
            assert_eq!(buf.line_of_char(at), line, "line of char {at}");
        }
        // And replaying the journal backwards gives back the original, exactly.
        for e in buf.take_journal().iter().rev() {
            buf.undo_edit(e);
        }
        assert_eq!(buf.to_text(), start);
    }

    #[test]
    fn reading_a_stream_decodes_the_way_lossy_does_across_block_boundaries() {
        // Every damaged shape, repeated until it straddles the 64 KB block edge
        // at every possible offset: a lone continuation byte, a truncated
        // three-byte character, a stray lead byte, and good multi-byte text.
        let pieces: [&[u8]; 5] = [
            "héllo 日本語 😀\n".as_bytes(),
            &[0x80],
            &[0xE6, 0x97],
            &[0xF0, 0x9F],
            b"plain ascii line\n",
        ];
        for shift in 0..8usize {
            let mut bytes = vec![b'x'; shift];
            let mut i = 0;
            while bytes.len() < 200_000 {
                bytes.extend_from_slice(pieces[i % pieces.len()]);
                i += 1;
            }
            // And a character cut off by the end of the file itself.
            bytes.extend_from_slice(&[0xE6, 0x97]);
            let want = String::from_utf8_lossy(&bytes).into_owned();
            let got = Buffer::from_reader(std::io::Cursor::new(&bytes)).unwrap();
            assert_eq!(got.to_text(), want, "shift {shift}");
        }
    }

    #[test]
    fn text_put_in_never_carries_a_carriage_return() {
        let mut b = Buffer::from("ab");
        b.insert(1, "x\r\ny\rz\n");
        assert_eq!(b, "ax\ny\nz\nb", "CRLF and lone CR both become a newline");
        // And undo takes back exactly what went in.
        for e in b.take_journal().iter().rev() {
            b.undo_edit(e);
        }
        assert_eq!(b, "ab");
    }

    #[test]
    fn windows_line_endings_are_read_as_newlines_even_across_block_edges() {
        // A CR at the very end of one 64 KB block and its LF at the start of the
        // next is still one line ending, at every offset.
        for shift in 0..6usize {
            let mut bytes = vec![b'x'; 65_534 + shift];
            bytes.extend_from_slice(b"\r\nafter\r\nlone\rcr\n");
            let (got, crlf) = Buffer::from_reader_lf(std::io::Cursor::new(&bytes)).unwrap();
            assert!(crlf, "shift {shift}");
            let want = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
            assert_eq!(got.to_text(), want, "shift {shift}");
        }
        let (plain, crlf) = Buffer::from_reader_lf(std::io::Cursor::new(b"a\nb\n")).unwrap();
        assert!(!crlf, "a file with only LF is not CRLF");
        assert_eq!(plain, "a\nb\n");
        let (edge, _) = Buffer::from_reader_lf(std::io::Cursor::new(b"ends with cr\r")).unwrap();
        assert_eq!(edge, "ends with cr\r", "a CR that ends the file is kept");
    }

    #[test]
    fn edits_are_journaled_and_can_be_replayed_backwards() {
        let mut b = Buffer::from("hello world");
        b.replace(0, 5, "goodbye");
        b.insert(7, ",");
        b.remove(9, 14);
        assert_eq!(b, "goodbye, ");
        let edits = b.take_journal();
        assert_eq!(edits.len(), 3);
        assert!(!b.has_journal());
        for e in edits.iter().rev() {
            b.undo_edit(e);
        }
        assert_eq!(b, "hello world");
        for e in &edits {
            b.redo_edit(e);
        }
        assert_eq!(b, "goodbye, ");
        assert!(!b.has_journal(), "replaying is not recorded");
    }
}
