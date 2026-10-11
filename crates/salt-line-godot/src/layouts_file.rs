//! The player's saved layouts and the editor's draft on disk, with no
//! engine in them (DESIGN §13.8, §13.11).
//!
//! # The slot file: `user://layouts.txt`
//!
//! ```text
//! layouts 1
//! layout 1 SL0J8…
//! layout 2 SL1M4… seed:DUNE:1
//! ```
//!
//! A header naming the file's [`FORMAT`], then a line per saved layout:
//! its slot (1 to [`MAX_SLOTS`]), its share code (§13.7), and optionally
//! where it was forked from (`editor::Origin::token`), which the share code
//! does not carry. Plain text the player owns, like the save: a layout is
//! already text, so the file is a list of codes.
//!
//! **Nothing a player made is thrown away by a build that does not
//! understand it.** A line this build cannot read (another tag, a slot out
//! of range or taken twice, a stray field) is kept verbatim and written
//! back after the slots; a code in a newer `LAYOUT_FORMAT`, or one that no
//! longer passes the gate after a sim change, keeps its slot and is shown
//! dimmed, never deleted. A file whose header is from a newer build is not
//! written at all ([`Slots::from_the_future`]), as the save is not
//! (`save::store`).
//!
//! Written whole through `durable::write_atomic` (the adapter's
//! `save::write`), as an `Effect::Store` the adapter reports back, so a
//! failed write says so and the draft it was saving is kept.
//!
//! # The draft: `user://draft.txt`
//!
//! ```text
//! draft 1 <mirror> <slot> <origin> <code>
//! draft 1 1 3 seed:DUNE:1 SL1M4…
//! ```
//!
//! The layout being edited, kept the way a versus match is (§11.6): one
//! newline-terminated line, replaced atomically after every edit that
//! changed it, set aside while a launch reads it, by `resume`'s file rules
//! at [`DRAFT_FILES`]. `slot` is the slot it was opened from or last saved
//! to and `origin` its fork label, `-` for none; `mirror` is the MIRROR
//! setting it was left with. It is cleared when the draft is saved or
//! discarded.

use salt_line_sim::layout::{CodeError, Layout};

use crate::editor::{Draft, Origin};
use crate::resume::{Paths, Stale};

pub const LAYOUTS_PATH: &str = "user://layouts.txt";
/// Written in full, then moved over `LAYOUTS_PATH`.
pub const LAYOUTS_TEMP_PATH: &str = "user://layouts.txt.new";

pub const DRAFT_PATH: &str = "user://draft.txt";
pub const DRAFT_TEMP_PATH: &str = "user://draft.txt.new";
/// Where the draft is moved while a launch reads it.
pub const DRAFT_LOADING_PATH: &str = "user://draft-loading.txt";

/// The draft file's paths, for `resume`'s file rules.
pub const DRAFT_FILES: Paths = Paths {
    main: DRAFT_PATH,
    temp: DRAFT_TEMP_PATH,
    loading: DRAFT_LOADING_PATH,
};

/// The slot file's format, its header's number.
pub const FORMAT: u32 = 1;

/// The draft line's format.
pub const DRAFT_FORMAT: u32 = 1;

/// Slots a player can save into: plenty for a family, few enough that the
/// list pages in a few screens.
pub const MAX_SLOTS: u8 = 32;

/// The longest file read back: 32 codes of the longest any layout can have
/// (`MAX_CODE_CHARS`), with room for kept lines. A larger file is damage.
pub const MAX_FILE_CHARS: usize = 256 * 1024;

/// A saved layout.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SlotEntry {
    /// 1 to [`MAX_SLOTS`].
    pub slot: u8,
    /// The share code as written, kept verbatim whether or not this build
    /// can read it.
    pub code: String,
    /// Where it was forked from, if the editor knew.
    pub origin: Option<Origin>,
    /// What followed the code on its line, verbatim, when this build could
    /// not read it as an origin (a newer build's, or a hand edit): kept and
    /// written back, never a reason to lose the layout before it.
    pub tail: Option<String>,
}

impl SlotEntry {
    /// Its layout, if this build reads its code.
    pub fn layout(&self) -> Result<Layout, CodeError> {
        Layout::from_code(&self.code)
    }
}

/// The slot file in memory.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Slots {
    /// By slot, ascending, each slot at most once.
    pub entries: Vec<SlotEntry>,
    /// Lines this build could not read, verbatim, in order.
    pub kept: Vec<String>,
    /// The header is from a newer build: shown, never written.
    pub from_the_future: bool,
    /// The file could not be read at all (too large, or with NULs a torn
    /// write leaves): never written over, so what is in it is not lost to
    /// the next save.
    pub damaged: bool,
}

impl Slots {
    /// Read the file. Total: any text is a `Slots`, a damaged or unknown
    /// line kept rather than lost.
    pub fn parse(text: &str) -> Slots {
        let mut out = Slots::default();
        if text.len() > MAX_FILE_CHARS || text.contains('\0') {
            out.damaged = true;
            return out;
        }
        let mut header_seen = false;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let mut f = line.split(' ');
            if !header_seen {
                if f.next() == Some("layouts") {
                    header_seen = true;
                    match f.next().and_then(canonical::<u32>) {
                        Some(v) if v > FORMAT => out.from_the_future = true,
                        Some(_) if f.next().is_none() => {}
                        // A damaged header is kept, and a good one written
                        // above it.
                        _ => out.kept.push(line.to_string()),
                    }
                    continue;
                }
                // No header: not a file this build wrote, but keep what it
                // can read.
                header_seen = true;
            }
            match parse_slot_line(line) {
                Some(e) if !out.entries.iter().any(|x| x.slot == e.slot) => out.entries.push(e),
                _ => out.kept.push(line.to_string()),
            }
        }
        out.entries.sort_by_key(|e| e.slot);
        out
    }

    /// The file's text: the header, a line per slot, then the kept lines.
    pub fn serialize(&self) -> String {
        let mut s = format!("layouts {FORMAT}\n");
        for e in &self.entries {
            s.push_str(&format!("layout {} {}", e.slot, e.code));
            if let Some(o) = &e.origin {
                s.push(' ');
                s.push_str(&o.token());
            } else if let Some(t) = &e.tail {
                s.push(' ');
                s.push_str(t);
            }
            s.push('\n');
        }
        for k in &self.kept {
            s.push_str(k);
            s.push('\n');
        }
        s
    }

    pub fn get(&self, slot: u8) -> Option<&SlotEntry> {
        self.entries.iter().find(|e| e.slot == slot)
    }

    /// Put `entry` in its slot, replacing what was there. Returns what was.
    pub fn put(&mut self, entry: SlotEntry) -> Option<SlotEntry> {
        let old = self.remove(entry.slot);
        self.entries.push(entry);
        self.entries.sort_by_key(|e| e.slot);
        old
    }

    /// Empty `slot`. Returns what was there.
    pub fn remove(&mut self, slot: u8) -> Option<SlotEntry> {
        let i = self.entries.iter().position(|e| e.slot == slot)?;
        Some(self.entries.remove(i))
    }

    /// The lowest empty slot, if any is left.
    pub fn free(&self) -> Option<u8> {
        (1..=MAX_SLOTS).find(|s| !self.taken(*s))
    }

    /// Whether `slot` holds anything: a layout, or a kept line that names
    /// it (a second line for a slot, or one whose code is not shaped like a
    /// code), which a save there would otherwise sit beside.
    pub fn taken(&self, slot: u8) -> bool {
        self.get(slot).is_some()
            || self.kept.iter().any(|k| {
                let mut f = k.split(' ');
                f.next() == Some("layout") && f.next().and_then(canonical::<u8>) == Some(slot)
            })
    }
}

/// One `layout <slot> <code> [origin]` line, if this build reads it.
fn parse_slot_line(line: &str) -> Option<SlotEntry> {
    let mut f = line.split(' ');
    if f.next()? != "layout" {
        return None;
    }
    let slot: u8 = canonical(f.next()?)?;
    if !(1..=MAX_SLOTS).contains(&slot) {
        return None;
    }
    let code = f.next()?;
    if !code_like(code) {
        return None;
    }
    let rest: Vec<&str> = f.collect();
    let origin = match rest.as_slice() {
        [t] => Origin::parse(t),
        _ => None,
    };
    let tail = (origin.is_none() && !rest.is_empty()).then(|| rest.join(" "));
    Some(SlotEntry {
        slot,
        code: code.to_string(),
        origin,
        tail,
    })
}

/// Text that is shaped like a share code: the prefix and base32 after it,
/// upper case, as the encoder writes. Whether it decodes is another matter
/// (a newer format, say), and the line keeps its slot either way.
fn code_like(t: &str) -> bool {
    use salt_line_sim::layout::{CODE_PREFIX, MAX_CODE_CHARS};
    t.len() <= MAX_CODE_CHARS
        && t.len() > CODE_PREFIX.len()
        && t.starts_with(CODE_PREFIX)
        && t[CODE_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
}

/// A number written only the way `Display` writes it.
fn canonical<T: std::str::FromStr + ToString>(tok: &str) -> Option<T> {
    let v: T = tok.parse().ok()?;
    (v.to_string() == tok).then_some(v)
}

/// Whether the slot file's text is a whole copy, for choosing between it
/// and its temporary file: a `layouts` header and no NUL.
pub fn sound(text: &str) -> bool {
    !text.contains('\0')
        && text
            .lines()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.split(' ').next() == Some("layouts"))
}

/// Four characters that tell layouts apart at a glance (§13.11): the top
/// twenty bits of the layout's digest in Crockford base32, so two players
/// can check by eye that they hold the same layout.
pub fn fingerprint(layout: &Layout) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let d = layout.digest();
    (0..4)
        .map(|i| ALPHABET[((d >> (59 - 5 * i)) & 31) as usize] as char)
        .collect()
}

// ------------------------------------------------------------------ draft

/// The draft as the draft file keeps it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SavedDraft {
    pub draft: Draft,
    pub mirror: bool,
}

impl SavedDraft {
    /// The module docs' line, newline-terminated. `None` for a layout the
    /// share code cannot hold (no edit makes one).
    pub fn to_line(&self) -> Option<String> {
        let code = self.draft.layout.to_code().ok()?;
        let slot = self
            .draft
            .slot
            .map_or_else(|| "-".to_string(), |s| s.to_string());
        let origin = self
            .draft
            .from
            .as_ref()
            .map_or_else(|| "-".to_string(), Origin::token);
        Some(format!(
            "draft {DRAFT_FORMAT} {} {slot} {origin} {code}\n",
            u8::from(self.mirror)
        ))
    }

    /// Read a draft line. The format is checked before anything after it.
    pub fn parse(text: &str) -> Result<SavedDraft, Stale> {
        if text.len() > MAX_FILE_CHARS || text.contains('\0') || !text.ends_with('\n') {
            return Err(Stale::Damaged);
        }
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let line = lines.next().ok_or(Stale::Damaged)?;
        if lines.next().is_some() {
            return Err(Stale::Damaged);
        }
        let mut f = line.split(' ');
        if f.next() != Some("draft") {
            return Err(Stale::Damaged);
        }
        let format: u32 = canonical(f.next().ok_or(Stale::Damaged)?).ok_or(Stale::Damaged)?;
        match format.cmp(&DRAFT_FORMAT) {
            std::cmp::Ordering::Less => return Err(Stale::OlderVersion),
            std::cmp::Ordering::Greater => return Err(Stale::NewerVersion),
            std::cmp::Ordering::Equal => {}
        }
        let mirror = match f.next() {
            Some("0") => false,
            Some("1") => true,
            _ => return Err(Stale::Damaged),
        };
        let slot = match f.next().ok_or(Stale::Damaged)? {
            "-" => None,
            t => {
                let s: u8 = canonical(t).ok_or(Stale::Damaged)?;
                if !(1..=MAX_SLOTS).contains(&s) {
                    return Err(Stale::Damaged);
                }
                Some(s)
            }
        };
        // An origin this build cannot read (a newer build's) is dropped,
        // never the draft with it.
        let from = match f.next().ok_or(Stale::Damaged)? {
            "-" => None,
            t => Origin::parse(t),
        };
        let code = f.next().ok_or(Stale::Damaged)?;
        if f.next().is_some() || !code_like(code) {
            return Err(Stale::Damaged);
        }
        let layout = Layout::from_code(code).map_err(code_stale)?;
        // A layout this editor cannot hold (another width, one gun) is a
        // later build's draft (challenges, say): kept for it, as a slot's
        // would be, never deleted.
        if layout.width() != salt_line_sim::layout::LAYOUT_WIDTH || !layout.is_duel() {
            return Err(Stale::NewerVersion);
        }
        Ok(SavedDraft {
            draft: Draft { layout, from, slot },
            mirror,
        })
    }
}

/// Why a share code in a file this build wrote cannot be read: a format
/// past this build's is a newer build's, one before it an older one's
/// (no build wrote one, but it is not this build's to read), anything else
/// damage.
pub fn code_stale(e: CodeError) -> Stale {
    match e {
        CodeError::Format(v) if v > salt_line_sim::layout::LAYOUT_FORMAT => Stale::NewerVersion,
        CodeError::Format(_) => Stale::OlderVersion,
        _ => Stale::Damaged,
    }
}

/// Whether a draft file's text is a whole copy: one well-formed line, of
/// this build or another.
pub fn draft_sound(text: &str) -> bool {
    matches!(
        SavedDraft::parse(text),
        Ok(_) | Err(Stale::OlderVersion | Stale::NewerVersion)
    )
}

#[cfg(test)]
mod tests;
