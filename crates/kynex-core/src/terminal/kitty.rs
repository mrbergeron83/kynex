//! Kitty graphics protocol (`\x1b_G...\x1b\\`) support for inline terminal images (#453).
//!
//! Architecture:
//! - [`InterceptPty`] wraps `alacritty_terminal::tty::Pty`, scanning PTY output through
//!   [`StreamFilter`] without copying normal text chunks.
//! - Query (`a=q`) and transmit-only (`a=t` / chunked `m=1`) commands are answered immediately.
//! - Display (`a=T`, `a=p`) and cursor-relative delete (`a=d` with `d=c`/`C`) commands inject a
//!   lightweight `OSC 2 knx-kg:<seq>` + `CSI 6n` (CPR) probe into the stream so the terminal's
//!   exact cursor `(line, col)` at that byte offset is captured synchronously with zero locks.
//! - Standard inline placements (`C=0`) also emit invisible `OSC 8 knx:img:<key>:<row>` cell
//!   anchors so images scroll smoothly with scrollback, clip partially at the viewport edge,
//!   hide on alt-screen, and vanish automatically on `clear`.
//! - Non-cursor-moving overlay placements (`C=1`, e.g. `yazi` / `ranger`) stay pinned to their
//!   screen coordinates without disturbing any text cells.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::read::ZlibDecoder;
use image::{ImageBuffer, Rgba, RgbaImage};
use polling::{Event as PollingEvent, PollMode, Poller};

use crate::file_view::media::{self, DRAW_SIDE, MAX_PIXELS};

/// Maximum raw APC payload buffered per Kitty graphics escape sequence (32 MiB).
const MAX_APC_BYTES: usize = 32 << 20;
/// Maximum total decoded RGBA bytes kept in one terminal's Kitty image cache (64 MiB).
const MAX_CACHE_BYTES: usize = 64 << 20;
/// Maximum distinct images cached per terminal.
const MAX_CACHED_IMAGES: usize = 128;
/// Maximum display rows for one inline image placement (prevents runaway linefeeds).
const MAX_IMAGE_ROWS: usize = 200;
/// Maximum raw file bytes read for `t=f` / `t=t` transmission mediums.
const MAX_FILE_BYTES: u64 = 32 << 20;

/// Title prefix used for synchronous cursor-position probes (`OSC 2 knx-kg:<seq> BEL` + `CSI 6n`).
pub(super) const PROBE_PREFIX: &str = "knx-kg:";
/// OSC 8 URI prefix used to anchor `C=0` inline placements to terminal grid rows.
pub(super) const ANCHOR_PREFIX: &str = "knx:img:";

/// A decoded Kitty graphics image stored in a terminal's cache.
#[derive(Clone, Debug)]
pub struct KittyImageData {
    pub id: u32,
    /// Monotonic generation counter incremented whenever `id` is re-transmitted.
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA pixels (scaled to <= `DRAW_SIDE` on longest edge).
    pub rgba: RgbaImage,
}

/// Source crop rectangle `(x, y, width, height)` in pixels.
pub type SrcRect = (u32, u32, u32, u32);

/// One visible inline image placement ready for the renderer to draw.
#[derive(Clone, Debug)]
pub struct PlacedImage {
    pub image_id: u32,
    pub image_gen: u64,
    pub data: Arc<KittyImageData>,
    /// Top-left cell on screen (may be negative when the top rows scrolled above the viewport).
    pub line: i32,
    pub col: usize,
    /// Display extent in terminal cells.
    pub cols: usize,
    pub rows: usize,
    /// Optional sub-cell X/Y offsets in pixels (`X`, `Y`).
    pub offset_x: u32,
    pub offset_y: u32,
    /// Source crop rectangle `(x, y, w, h)` in pixels, if partial.
    pub src_rect: Option<SrcRect>,
    /// Kitty z-index: `< 0` renders behind text, `>= 0` renders above text.
    pub z_index: i32,
}

/// Callback invoked by [`KittyStore`] to send Kitty protocol replies (`\x1b_G...;OK\x1b\\`) back to the PTY.
pub(super) type ReplyFn = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

/// Parsed control parameters from a Kitty graphics APC command (`key=value,...`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KittyParams {
    pub action: Option<char>,
    pub format: Option<u32>,
    pub medium: Option<char>,
    pub compression: Option<char>,
    pub id: u32,
    pub image_number: u32,
    pub placement_id: u32,
    pub width: u32,
    pub height: u32,
    pub cols: Option<u32>,
    pub rows: Option<u32>,
    pub src_x: u32,
    pub src_y: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub z_index: i32,
    pub no_cursor_move: bool,
    pub more: bool,
    pub quiet: u8,
    pub delete: Option<char>,
}

/// Splits a raw Kitty APC body (everything between `\x1b_G` and ST) into control params + payload.
pub fn parse_command(raw: &[u8]) -> Result<(KittyParams, &[u8]), String> {
    let (ctrl, payload) = match raw.iter().position(|&b| b == b';') {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, &[][..]),
    };
    let ctrl_str = std::str::from_utf8(ctrl).map_err(|_| "invalid UTF-8 in Kitty control data".to_string())?;
    let mut p = KittyParams::default();
    if !ctrl_str.is_empty() {
        for part in ctrl_str.split(',') {
            if part.is_empty() {
                continue;
            }
            let Some((k, v)) = part.split_once('=') else {
                return Err(format!("malformed key/value `{part}`"));
            };
            match k {
                "a" => p.action = v.chars().next(),
                "f" => p.format = v.parse().ok(),
                "t" => p.medium = v.chars().next(),
                "o" => p.compression = v.chars().next(),
                "i" => p.id = v.parse().unwrap_or(0),
                "I" => p.image_number = v.parse().unwrap_or(0),
                "p" => p.placement_id = v.parse().unwrap_or(0),
                "s" => p.width = v.parse().unwrap_or(0),
                "v" => p.height = v.parse().unwrap_or(0),
                "c" => p.cols = v.parse().ok().filter(|&n| n > 0),
                "r" => p.rows = v.parse().ok().filter(|&n| n > 0),
                "x" => p.src_x = v.parse().unwrap_or(0),
                "y" => p.src_y = v.parse().unwrap_or(0),
                "w" => p.src_w = v.parse().unwrap_or(0),
                "h" => p.src_h = v.parse().unwrap_or(0),
                "X" => p.offset_x = v.parse().unwrap_or(0),
                "Y" => p.offset_y = v.parse().unwrap_or(0),
                "z" => p.z_index = v.parse().unwrap_or(0),
                "C" => p.no_cursor_move = v == "1",
                "m" => p.more = v == "1",
                "q" => p.quiet = v.parse().unwrap_or(0),
                "d" => p.delete = v.chars().next(),
                _ => {}
            }
        }
    }
    Ok((p, payload))
}

/// Parses a `CSI <row>;<col>R` cursor position report into 0-based `(line, col)`.
pub(super) fn parse_cpr(text: &str) -> Option<(usize, usize)> {
    let body = text.strip_prefix("\x1b[")?.strip_suffix('R')?;
    let (r, c) = body.split_once(';')?;
    let row: usize = r.parse().ok()?;
    let col: usize = c.parse().ok()?;
    Some((row.saturating_sub(1), col.saturating_sub(1)))
}

/// Parses an OSC 8 anchor URI `knx:img:<key>:<row_offset>` into `(placement_key, row_offset)`.
pub(super) fn parse_anchor_uri(uri: &str) -> Option<(u64, usize)> {
    let rest = uri.strip_prefix(ANCHOR_PREFIX)?;
    let (k, r) = rest.split_once(':')?;
    Some((k.parse().ok()?, r.parse().ok()?))
}

/// Incremental byte-stream filter separating normal terminal bytes from Kitty APC sequences.
#[derive(Default)]
pub struct StreamFilter {
    state: FilterState,
    buf: Vec<u8>,
    overflowed: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum FilterState {
    #[default]
    Normal,
    Esc,
    ApcStart,
    Kitty,
    KittyEsc,
}

/// One output segment yielded by [`StreamFilter::feed`].
pub enum StreamChunk<'a> {
    Normal(&'a [u8]),
    Kitty(Vec<u8>),
}

impl StreamFilter {
    /// Scans `input` and invokes `emit` for normal slices and complete Kitty APC payloads.
    pub fn feed<'a>(&mut self, input: &'a [u8], mut emit: impl FnMut(StreamChunk<'a>)) {
        if self.state == FilterState::Normal && !input.contains(&0x1b) {
            if !input.is_empty() {
                emit(StreamChunk::Normal(input));
            }
            return;
        }
        let mut normal_start: Option<usize> = None;
        let mut i = 0;
        while i < input.len() {
            let b = input[i];
            match self.state {
                FilterState::Normal => {
                    if b == 0x1b {
                        if let Some(s) = normal_start.take() {
                            emit(StreamChunk::Normal(&input[s..i]));
                        }
                        self.state = FilterState::Esc;
                    } else if normal_start.is_none() {
                        normal_start = Some(i);
                    }
                }
                FilterState::Esc => {
                    if b == b'_' {
                        self.state = FilterState::ApcStart;
                    } else {
                        emit(StreamChunk::Normal(b"\x1b"));
                        self.state = FilterState::Normal;
                        if b == 0x1b {
                            self.state = FilterState::Esc;
                        } else {
                            normal_start = Some(i);
                        }
                    }
                }
                FilterState::ApcStart => {
                    if b == b'G' {
                        self.buf.clear();
                        self.overflowed = false;
                        self.state = FilterState::Kitty;
                    } else {
                        emit(StreamChunk::Normal(b"\x1b_"));
                        self.state = FilterState::Normal;
                        if b == 0x1b {
                            self.state = FilterState::Esc;
                        } else {
                            normal_start = Some(i);
                        }
                    }
                }
                FilterState::Kitty => {
                    if b == 0x1b {
                        self.state = FilterState::KittyEsc;
                    } else if b == 0x07 {
                        let payload = std::mem::take(&mut self.buf);
                        let ok = !self.overflowed;
                        self.overflowed = false;
                        self.state = FilterState::Normal;
                        if ok {
                            emit(StreamChunk::Kitty(payload));
                        }
                    } else if !self.overflowed {
                        if self.buf.len() < MAX_APC_BYTES {
                            self.buf.push(b);
                        } else {
                            self.buf.clear();
                            self.overflowed = true;
                        }
                    }
                }
                FilterState::KittyEsc => {
                    if b == b'\\' {
                        let payload = std::mem::take(&mut self.buf);
                        let ok = !self.overflowed;
                        self.overflowed = false;
                        self.state = FilterState::Normal;
                        if ok {
                            emit(StreamChunk::Kitty(payload));
                        }
                    } else {
                        if !self.overflowed {
                            if self.buf.len() + 2 <= MAX_APC_BYTES {
                                self.buf.push(0x1b);
                                self.buf.push(b);
                            } else {
                                self.buf.clear();
                                self.overflowed = true;
                            }
                        }
                        self.state = if b == 0x1b { FilterState::KittyEsc } else { FilterState::Kitty };
                    }
                }
            }
            i += 1;
        }
        if let Some(s) = normal_start {
            emit(StreamChunk::Normal(&input[s..]));
        }
    }
}

struct ChunkAssembly {
    params: KittyParams,
    payload: Vec<u8>,
    overflowed: bool,
}

#[derive(Clone)]
struct Placement {
    key: u64,
    image_id: u32,
    placement_id: u32,
    cols: usize,
    rows: usize,
    offset_x: u32,
    offset_y: u32,
    src_rect: Option<(u32, u32, u32, u32)>,
    z_index: i32,
    /// `true` for `C=0` placements tracked via `knx:img:<key>:<row>` cell anchors.
    anchored: bool,
    /// For `C=1` overlay placements: screen coordinates and scrollback state when placed.
    screen_line: usize,
    screen_col: usize,
    alt_screen: bool,
    history_at_place: usize,
}

enum PendingOp {
    Place(Placement),
    Delete { selector: char, free_images: bool, id: u32, placement_id: u32, x: u32, y: u32, z: i32 },
}

/// Per-terminal Kitty graphics image cache and active placements.
pub(super) struct KittyStore {
    images: HashMap<u32, Arc<KittyImageData>>,
    image_order: VecDeque<u32>,
    total_bytes: usize,
    next_auto_id: u32,
    next_gen: u64,
    next_key: u64,
    next_seq: u64,
    assembly: Option<ChunkAssembly>,
    placements: Vec<Placement>,
    pending_ops: HashMap<u64, PendingOp>,
    /// Packed `(cell_w << 16) | cell_h`, updated by `Terminal::resize_with_cell`.
    cell_px: Arc<AtomicU32>,
    /// Terminal columns, updated on resize.
    term_cols: Arc<AtomicU32>,
    /// Last known alt-screen mode and scrollback history size (updated on snapshot).
    last_alt_screen: bool,
    last_history_size: usize,
}

impl KittyStore {
    pub(super) fn new(cell_px: Arc<AtomicU32>, term_cols: Arc<AtomicU32>) -> Self {
        Self {
            images: HashMap::new(),
            image_order: VecDeque::new(),
            total_bytes: 0,
            next_auto_id: 1,
            next_gen: 1,
            next_key: 1,
            next_seq: 1,
            assembly: None,
            placements: Vec::new(),
            pending_ops: HashMap::new(),
            cell_px,
            term_cols,
            last_alt_screen: false,
            last_history_size: 0,
        }
    }

    fn cell_dims(&self) -> (u32, u32) {
        let packed = self.cell_px.load(Ordering::Relaxed);
        let w = (packed >> 16).max(1);
        let h = (packed & 0xffff).max(1);
        (w, h)
    }

    fn cols_limit(&self) -> usize {
        (self.term_cols.load(Ordering::Relaxed) as usize).max(2)
    }

    /// Processes one complete Kitty APC payload (`raw`, between `\x1b_G` and ST).
    /// Any probe / anchor bytes to inject into the terminal byte stream are appended to `inject`.
    /// Returns `true` if visual state changed and the UI should be woken.
    pub(super) fn on_apc(&mut self, raw: &[u8], inject: &mut Vec<u8>, reply: &dyn Fn(Vec<u8>)) -> bool {
        let (parsed, payload) = match parse_command(raw) {
            Ok(v) => v,
            Err(e) => {
                send_reply(reply, 0, 0, 0, Err(&e));
                return false;
            }
        };

        // Handle multi-chunk assembly (`m=1` ... `m=0`).
        let (params, full_payload): (KittyParams, Vec<u8>) = if let Some(mut asm) = self.assembly.take() {
            if !asm.overflowed {
                if asm.payload.len() + payload.len() <= MAX_APC_BYTES {
                    asm.payload.extend_from_slice(payload);
                } else {
                    asm.payload.clear();
                    asm.overflowed = true;
                }
            }
            if parsed.more {
                self.assembly = Some(asm);
                return false;
            }
            if asm.overflowed {
                send_reply(
                    reply,
                    asm.params.id,
                    asm.params.placement_id,
                    asm.params.quiet,
                    Err("ENOSPC:image too large"),
                );
                return false;
            }
            (asm.params, asm.payload)
        } else if parsed.more {
            let mut buf = Vec::with_capacity(payload.len());
            buf.extend_from_slice(payload);
            self.assembly = Some(ChunkAssembly { params: parsed, payload: buf, overflowed: false });
            return false;
        } else {
            (parsed, payload.to_vec())
        };

        let action = params.action.unwrap_or('t');
        match action {
            'q' => {
                // Query support: validate basic decoding parameters without storing/displaying.
                let res = decode_kitty_image(&params, &full_payload).map(|_| ());
                send_reply(
                    reply,
                    params.id,
                    params.placement_id,
                    params.quiet,
                    res.as_ref().map(|_| ()).map_err(String::as_str),
                );
                false
            }
            't' => match self.store_image(&params, &full_payload) {
                Ok(id) => {
                    send_reply(reply, id, params.placement_id, params.quiet, Ok(()));
                    false
                }
                Err(e) => {
                    send_reply(reply, params.id, params.placement_id, params.quiet, Err(&e));
                    false
                }
            },
            'T' => match self.store_image(&params, &full_payload) {
                Ok(id) => {
                    let ok = self.schedule_placement(id, &params, inject);
                    send_reply(
                        reply,
                        id,
                        params.placement_id,
                        params.quiet,
                        ok.as_ref().map(|_| ()).map_err(String::as_str),
                    );
                    ok.is_ok()
                }
                Err(e) => {
                    send_reply(reply, params.id, params.placement_id, params.quiet, Err(&e));
                    false
                }
            },
            'p' => {
                let id = params.id;
                let res = self.schedule_placement(id, &params, inject);
                send_reply(
                    reply,
                    id,
                    params.placement_id,
                    params.quiet,
                    res.as_ref().map(|_| ()).map_err(String::as_str),
                );
                res.is_ok()
            }
            'd' => self.handle_delete(&params, inject),
            _ => false,
        }
    }

    fn store_image(&mut self, params: &KittyParams, payload: &[u8]) -> Result<u32, String> {
        let rgba = decode_kitty_image(params, payload)?;
        let id = if params.id != 0 {
            params.id
        } else {
            let mut id = self.next_auto_id;
            while id == 0 || self.images.contains_key(&id) {
                id = id.wrapping_add(1).max(1);
            }
            self.next_auto_id = id.wrapping_add(1).max(1);
            id
        };
        let generation = self.next_gen;
        self.next_gen += 1;
        let (width, height) = rgba.dimensions();
        let bytes = rgba.as_raw().len();
        if let Some(old) = self.images.remove(&id) {
            self.total_bytes = self.total_bytes.saturating_sub(old.rgba.as_raw().len());
            self.image_order.retain(|&x| x != id);
        }
        self.images.insert(id, Arc::new(KittyImageData { id, generation, width, height, rgba }));
        self.image_order.push_back(id);
        self.total_bytes += bytes;
        self.evict_over_limit(id);
        Ok(id)
    }

    fn evict_over_limit(&mut self, keep_id: u32) {
        while (self.total_bytes > MAX_CACHE_BYTES || self.images.len() > MAX_CACHED_IMAGES)
            && self.image_order.len() > 1
        {
            let Some(oldest) = self.image_order.pop_front() else { break };
            if oldest == keep_id {
                self.image_order.push_back(oldest);
                break;
            }
            if let Some(old) = self.images.remove(&oldest) {
                self.total_bytes = self.total_bytes.saturating_sub(old.rgba.as_raw().len());
            }
            self.placements.retain(|p| p.image_id != oldest);
        }
    }

    fn schedule_placement(&mut self, id: u32, params: &KittyParams, inject: &mut Vec<u8>) -> Result<(), String> {
        let img = self.images.get(&id).ok_or_else(|| format!("ENOENT:unknown image {id}"))?;
        let (src_w, src_h, src_rect) = resolve_src_rect(img.width, img.height, params);
        let (cell_w, cell_h) = self.cell_dims();
        let (cols, rows) = resolve_cell_size(src_w, src_h, params.cols, params.rows, cell_w, cell_h, self.cols_limit());

        let key = self.next_key;
        self.next_key += 1;
        let seq = self.next_seq;
        self.next_seq += 1;

        let anchored = !params.no_cursor_move;
        let draft = Placement {
            key,
            image_id: id,
            placement_id: params.placement_id,
            cols,
            rows,
            offset_x: params.offset_x,
            offset_y: params.offset_y,
            src_rect,
            z_index: params.z_index,
            anchored,
            screen_line: 0,
            screen_col: 0,
            alt_screen: self.last_alt_screen,
            history_at_place: self.last_history_size,
        };
        self.pending_ops.insert(seq, PendingOp::Place(draft));

        // Emit synchronous cursor-position probe right at the placement point.
        let _ = write!(inject, "\x1b]2;{PROBE_PREFIX}{seq}\x07\x1b[6n");

        // Standard inline placements (`C=0`) occupy `rows` lines and anchor each row to its grid
        // cell via an internal `knx:img:` OSC 8 URI so scrollback, clipping, and `clear` work
        // automatically.
        if anchored {
            for row_i in 0..rows {
                let _ = write!(inject, "\x1b]8;;{ANCHOR_PREFIX}{key}:{row_i}\x1b\\ \x1b]8;;\x1b\\");
                if row_i + 1 < rows {
                    inject.extend_from_slice(b"\x08\n");
                }
            }
            if cols > 1 {
                let _ = write!(inject, "\x1b[{}C", cols - 1);
            }
        }
        Ok(())
    }

    fn handle_delete(&mut self, params: &KittyParams, inject: &mut Vec<u8>) -> bool {
        let sel = params.delete.unwrap_or('a');
        let free_images = sel.is_ascii_uppercase();
        let lower = sel.to_ascii_lowercase();
        if lower == 'c' {
            let seq = self.next_seq;
            self.next_seq += 1;
            self.pending_ops.insert(
                seq,
                PendingOp::Delete {
                    selector: lower,
                    free_images,
                    id: params.id,
                    placement_id: params.placement_id,
                    x: params.src_x,
                    y: params.src_y,
                    z: params.z_index,
                },
            );
            let _ = write!(inject, "\x1b]2;{PROBE_PREFIX}{seq}\x07\x1b[6n");
            return true;
        }
        self.apply_delete(
            lower,
            free_images,
            params.id,
            params.placement_id,
            params.src_x,
            params.src_y,
            params.z_index,
            None,
        )
    }

    /// Completes a queued placement or cursor-relative delete once `Listener` receives `CSI row;colR`.
    pub(super) fn resolve_cursor(&mut self, seq: u64, line: usize, col: usize) {
        let Some(op) = self.pending_ops.remove(&seq) else { return };
        match op {
            PendingOp::Place(mut p) => {
                p.screen_line = line;
                p.screen_col = col;
                p.alt_screen = self.last_alt_screen;
                p.history_at_place = self.last_history_size;
                // Replacing an existing placement with the same explicit `(image_id, placement_id)`.
                if p.placement_id != 0 {
                    self.placements.retain(|old| !(old.image_id == p.image_id && old.placement_id == p.placement_id));
                }
                self.placements.push(p);
            }
            PendingOp::Delete { selector, free_images, id, placement_id, x, y, z } => {
                self.apply_delete(selector, free_images, id, placement_id, x, y, z, Some((line, col)));
            }
        }
    }

    fn apply_delete(
        &mut self,
        selector: char,
        free_images: bool,
        id: u32,
        placement_id: u32,
        x: u32,
        y: u32,
        z: i32,
        cursor: Option<(usize, usize)>,
    ) -> bool {
        let before = self.placements.len();
        let mut freed_ids: Vec<u32> = Vec::new();
        match selector {
            'a' => {
                if free_images {
                    self.images.clear();
                    self.image_order.clear();
                    self.total_bytes = 0;
                }
                self.placements.clear();
            }
            'i' => {
                self.placements.retain(|p| {
                    let drop = p.image_id == id && (placement_id == 0 || p.placement_id == placement_id);
                    if drop && free_images {
                        freed_ids.push(p.image_id);
                    }
                    !drop
                });
                if free_images && placement_id == 0 && id != 0 {
                    freed_ids.push(id);
                }
            }
            'p' => {
                let target_col = x.saturating_sub(1) as usize;
                let target_row = y.saturating_sub(1) as usize;
                self.placements.retain(|p| {
                    let hit = target_row >= p.screen_line
                        && target_row < p.screen_line + p.rows
                        && target_col >= p.screen_col
                        && target_col < p.screen_col + p.cols;
                    if hit && free_images {
                        freed_ids.push(p.image_id);
                    }
                    !hit
                });
            }
            'c' => {
                if let Some((crow, ccol)) = cursor {
                    self.placements.retain(|p| {
                        let hit = crow >= p.screen_line
                            && crow < p.screen_line + p.rows
                            && ccol >= p.screen_col
                            && ccol < p.screen_col + p.cols;
                        if hit && free_images {
                            freed_ids.push(p.image_id);
                        }
                        !hit
                    });
                }
            }
            'x' => {
                let target_col = x.saturating_sub(1) as usize;
                self.placements.retain(|p| !(target_col >= p.screen_col && target_col < p.screen_col + p.cols));
            }
            'y' => {
                let target_row = y.saturating_sub(1) as usize;
                self.placements.retain(|p| !(target_row >= p.screen_line && target_row < p.screen_line + p.rows));
            }
            'z' => {
                self.placements.retain(|p| p.z_index != z);
            }
            'n' => {
                if let Some(last) = self.placements.pop()
                    && free_images
                {
                    freed_ids.push(last.image_id);
                }
            }
            _ => {}
        }
        for fid in freed_ids {
            if !self.placements.iter().any(|p| p.image_id == fid)
                && let Some(old) = self.images.remove(&fid)
            {
                self.total_bytes = self.total_bytes.saturating_sub(old.rgba.as_raw().len());
                self.image_order.retain(|&k| k != fid);
            }
        }
        self.placements.len() != before
    }

    /// Synchronises alt-screen transitions and returns visible placements for the current frame.
    pub(super) fn visible_images(
        &mut self,
        anchored_pos: &HashMap<u64, (i32, usize)>,
        alt_screen: bool,
        history_size: usize,
        display_offset: usize,
        screen_lines: usize,
    ) -> Vec<PlacedImage> {
        // Leaving alt-screen drops any alt-screen placements.
        if self.last_alt_screen && !alt_screen {
            self.placements.retain(|p| !p.alt_screen);
        }
        self.last_alt_screen = alt_screen;
        self.last_history_size = history_size;

        let mut out = Vec::new();
        for p in &self.placements {
            let Some(data) = self.images.get(&p.image_id) else { continue };
            let (line, col) = if p.anchored {
                let Some(&(l, c)) = anchored_pos.get(&p.key) else { continue };
                (l, c)
            } else {
                if p.alt_screen != alt_screen {
                    continue;
                }
                let vis_line = if alt_screen {
                    p.screen_line as i32
                } else {
                    let scrolled = history_size.saturating_sub(p.history_at_place) as i32;
                    p.screen_line as i32 - scrolled + display_offset as i32
                };
                if vis_line + p.rows as i32 <= 0 || vis_line >= screen_lines as i32 {
                    continue;
                }
                (vis_line, p.screen_col)
            };
            out.push(PlacedImage {
                image_id: data.id,
                image_gen: data.generation,
                data: Arc::clone(data),
                line,
                col,
                cols: p.cols,
                rows: p.rows,
                offset_x: p.offset_x,
                offset_y: p.offset_y,
                src_rect: p.src_rect,
                z_index: p.z_index,
            });
        }
        out.sort_by_key(|p| p.z_index);
        out
    }
}

fn send_reply(reply: &dyn Fn(Vec<u8>), id: u32, placement_id: u32, quiet: u8, result: Result<(), &str>) {
    let ok = result.is_ok();
    if (ok && quiet >= 1) || (!ok && quiet >= 2) {
        return;
    }
    if id == 0 && ok {
        // Per Kitty spec: omit OK responses when no image id was supplied, unless querying (`a=q`).
    }
    let mut head = String::from("\x1b_G");
    if id != 0 {
        let _ = std::fmt::Write::write_fmt(&mut head, format_args!("i={id}"));
    }
    if placement_id != 0 {
        if id != 0 {
            head.push(',');
        }
        let _ = std::fmt::Write::write_fmt(&mut head, format_args!("p={placement_id}"));
    }
    head.push(';');
    match result {
        Ok(()) => head.push_str("OK"),
        Err(msg) => {
            if !msg.contains(':') {
                head.push_str("EINVAL:");
            }
            head.push_str(msg);
        }
    }
    head.push_str("\x1b\\");
    reply(head.into_bytes());
}

fn decode_kitty_image(params: &KittyParams, payload: &[u8]) -> Result<RgbaImage, String> {
    let cleaned: Vec<u8> = payload.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    let decoded = BASE64.decode(&cleaned).map_err(|_| "invalid base64 payload".to_string())?;
    let medium = params.medium.unwrap_or('d');
    let raw_bytes = match medium {
        'd' => decoded,
        'f' | 't' => {
            let path_str = std::str::from_utf8(&decoded).map_err(|_| "invalid file path UTF-8".to_string())?;
            let path = Path::new(path_str);
            let meta = std::fs::metadata(path).map_err(|e| format!("ENOENT:{e}"))?;
            if meta.len() > MAX_FILE_BYTES {
                return Err("ENOSPC:image file exceeds 32 MiB limit".into());
            }
            let data = std::fs::read(path).map_err(|e| format!("EIO:{e}"))?;
            if medium == 't' && is_safe_temp_file(path) {
                let _ = std::fs::remove_file(path);
            }
            data
        }
        other => return Err(format!("ENOTSUP:unsupported transmission medium `{other}`")),
    };

    let bytes = match params.compression {
        None => raw_bytes,
        Some('z') => {
            let mut dec = ZlibDecoder::new(raw_bytes.as_slice()).take(MAX_PIXELS * 4 + 1);
            let mut out = Vec::new();
            dec.read_to_end(&mut out).map_err(|e| format!("zlib error: {e}"))?;
            if out.len() as u64 > MAX_PIXELS * 4 {
                return Err("ENOSPC:decompressed image too large".into());
            }
            out
        }
        Some(other) => return Err(format!("ENOTSUP:unsupported compression `{other}`")),
    };

    match params.format.unwrap_or(32) {
        100 => media::decode_image(&bytes).map(|p| p.rgba),
        32 => {
            let (w, h) = (params.width, params.height);
            if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
                return Err("invalid RGBA dimensions".into());
            }
            let expected =
                (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).ok_or("size overflow")?;
            if bytes.len() < expected {
                return Err("RGBA payload shorter than width*height*4".into());
            }
            let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, bytes[..expected].to_vec())
                .ok_or("invalid RGBA buffer")?;
            Ok(fit_rgba(img))
        }
        24 => {
            let (w, h) = (params.width, params.height);
            if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
                return Err("invalid RGB dimensions".into());
            }
            let expected =
                (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(3)).ok_or("size overflow")?;
            if bytes.len() < expected {
                return Err("RGB payload shorter than width*height*3".into());
            }
            let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
            for px in bytes[..expected].as_chunks::<3>().0 {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            let img = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(w, h, rgba).ok_or("invalid RGB buffer")?;
            Ok(fit_rgba(img))
        }
        other => Err(format!("ENOTSUP:unsupported image format {other}")),
    }
}

fn fit_rgba(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= DRAW_SIDE {
        return img;
    }
    let scale = DRAW_SIDE as f32 / w.max(h) as f32;
    let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    image::DynamicImage::ImageRgba8(img).resize_exact(nw, nh, image::imageops::FilterType::Triangle).to_rgba8()
}

/// Only auto-deletes `t=t` files when they live in the OS temp folder or carry the Kitty marker name.
fn is_safe_temp_file(path: &Path) -> bool {
    let name_ok = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains("tty-graphics-protocol"));
    let temp = std::env::temp_dir();
    let in_temp = path.starts_with(&temp)
        || (std::fs::canonicalize(path)
            .ok()
            .zip(std::fs::canonicalize(&temp).ok())
            .is_some_and(|(a, b)| a.starts_with(&b)));
    name_ok && in_temp
}

fn resolve_src_rect(img_w: u32, img_h: u32, p: &KittyParams) -> (u32, u32, Option<SrcRect>) {
    if p.src_x == 0 && p.src_y == 0 && p.src_w == 0 && p.src_h == 0 {
        return (img_w.max(1), img_h.max(1), None);
    }
    let x = p.src_x.min(img_w.saturating_sub(1));
    let y = p.src_y.min(img_h.saturating_sub(1));
    let w = if p.src_w == 0 { img_w.saturating_sub(x) } else { p.src_w.min(img_w.saturating_sub(x)) }.max(1);
    let h = if p.src_h == 0 { img_h.saturating_sub(y) } else { p.src_h.min(img_h.saturating_sub(y)) }.max(1);
    if x == 0 && y == 0 && w == img_w && h == img_h { (w, h, None) } else { (w, h, Some((x, y, w, h))) }
}

fn resolve_cell_size(
    src_w: u32,
    src_h: u32,
    cols_opt: Option<u32>,
    rows_opt: Option<u32>,
    cell_w: u32,
    cell_h: u32,
    max_cols: usize,
) -> (usize, usize) {
    let cw = cell_w.max(1) as f64;
    let ch = cell_h.max(1) as f64;
    let sw = src_w.max(1) as f64;
    let sh = src_h.max(1) as f64;
    let (cols, rows) = match (cols_opt, rows_opt) {
        (Some(c), Some(r)) => (c as usize, r as usize),
        (Some(c), None) => {
            let r = ((c as f64 * cw * sh) / (sw * ch)).ceil() as usize;
            (c as usize, r)
        }
        (None, Some(r)) => {
            let c = ((r as f64 * ch * sw) / (sh * cw)).ceil() as usize;
            (c, r as usize)
        }
        (None, None) => {
            let mut c = (sw / cw).ceil() as usize;
            let mut r = (sh / ch).ceil() as usize;
            if c > max_cols {
                let scale = max_cols as f64 / c as f64;
                c = max_cols;
                r = (r as f64 * scale).ceil() as usize;
            }
            (c, r)
        }
    };
    (cols.clamp(1, max_cols), rows.clamp(1, MAX_IMAGE_ROWS))
}

/// Wrapper around `alacritty_terminal::tty::Pty` that intercepts Kitty graphics APC sequences
/// on the read side while forwarding polling, writing, resizing, and child exit unchanged.
pub(super) struct InterceptPty<P: EventedPty> {
    pty: P,
    state: InterceptState,
}

struct InterceptState {
    filter: StreamFilter,
    kitty: Arc<Mutex<KittyStore>>,
    reply: ReplyFn,
    wake: super::Wake,
    scratch: Vec<u8>,
    out: Vec<u8>,
    out_pos: usize,
}

struct InterceptReader<'a, R: Read> {
    inner: &'a mut R,
    state: &'a mut InterceptState,
}

impl<R: Read> Read for InterceptReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.state.out_pos < self.state.out.len() {
                let avail = &self.state.out[self.state.out_pos..];
                let n = avail.len().min(buf.len());
                buf[..n].copy_from_slice(&avail[..n]);
                self.state.out_pos += n;
                if self.state.out_pos == self.state.out.len() {
                    self.state.out.clear();
                    self.state.out_pos = 0;
                }
                return Ok(n);
            }
            let cap = self.state.scratch.len().min(buf.len().max(4096));
            let got = self.inner.read(&mut self.state.scratch[..cap])?;
            if got == 0 {
                return Ok(0);
            }
            let mut wake_needed = false;
            // Fast path: when filter is in Normal state and chunk has no ESC, bypass copy.
            if self.state.filter.state == FilterState::Normal && !self.state.scratch[..got].contains(&0x1b) {
                let n = got.min(buf.len());
                buf[..n].copy_from_slice(&self.state.scratch[..n]);
                if n < got {
                    self.state.out.extend_from_slice(&self.state.scratch[n..got]);
                    self.state.out_pos = 0;
                }
                return Ok(n);
            }
            let out = &mut self.state.out;
            let kitty = &self.state.kitty;
            let reply = &self.state.reply;
            self.state.filter.feed(&self.state.scratch[..got], |chunk| match chunk {
                StreamChunk::Normal(bytes) => out.extend_from_slice(bytes),
                StreamChunk::Kitty(raw) => {
                    if kitty.lock().unwrap().on_apc(&raw, out, reply.as_ref()) {
                        wake_needed = true;
                    }
                }
            });
            if wake_needed && self.state.out.is_empty() {
                (self.state.wake)();
            }
        }
    }
}

impl<P: EventedPty> InterceptPty<P> {
    pub(super) fn new(pty: P, kitty: Arc<Mutex<KittyStore>>, reply: ReplyFn, wake: super::Wake) -> Self {
        Self {
            pty,
            state: InterceptState {
                filter: StreamFilter::default(),
                kitty,
                reply,
                wake,
                scratch: vec![0u8; 64 * 1024],
                out: Vec::new(),
                out_pos: 0,
            },
        }
    }
}

impl<P: EventedPty> Read for InterceptPty<P> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut r = InterceptReader { inner: self.pty.reader(), state: &mut self.state };
        r.read(buf)
    }
}

impl<P: EventedPty + OnResize> OnResize for InterceptPty<P> {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size);
    }
}

impl<P: EventedPty> EventedReadWrite for InterceptPty<P> {
    type Reader = Self;
    type Writer = P::Writer;

    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: PollingEvent, poll_opts: PollMode) -> io::Result<()> {
        unsafe { self.pty.register(poll, interest, poll_opts) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, interest: PollingEvent, poll_opts: PollMode) -> io::Result<()> {
        self.pty.reregister(poll, interest, poll_opts)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.pty.writer()
    }
}

impl<P: EventedPty> EventedPty for InterceptPty<P> {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

/// Fuzz entry point for the Kitty graphics stream filter and command parser.
pub(crate) fn fuzz(bytes: &[u8]) {
    let mut filter = StreamFilter::default();
    for chunk in bytes.chunks(17) {
        filter.feed(chunk, |seg| {
            if let StreamChunk::Kitty(raw) = seg {
                let _ = parse_command(&raw);
            }
        });
    }
    let _ = parse_command(bytes);
    if let Ok(s) = std::str::from_utf8(bytes) {
        let _ = parse_cpr(s);
        let _ = parse_anchor_uri(s);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use alacritty_terminal::Term;
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::terminal::Size;

    fn png_b64(w: u32, h: u32) -> String {
        let mut out = Vec::new();
        let img = RgbaImage::from_pixel(w, h, Rgba([255, 0, 128, 255]));
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        BASE64.encode(out)
    }

    fn make_store() -> KittyStore {
        let cell = Arc::new(AtomicU32::new((8 << 16) | 16));
        let cols = Arc::new(AtomicU32::new(80));
        KittyStore::new(cell, cols)
    }

    #[test]
    fn stream_filter_extracts_kitty_apc_across_chunk_boundaries() {
        let mut f = StreamFilter::default();
        let input = b"before\x1b_Ga=q,i=7;AAAA\x1b\\middle\x1b_Ga=d,d=a\x07after\x1b[31mred";
        let mut normal = Vec::new();
        let mut kitty = Vec::new();
        // Feed 3 bytes at a time to stress every state boundary.
        for chunk in input.chunks(3) {
            f.feed(chunk, |seg| match seg {
                StreamChunk::Normal(b) => normal.extend_from_slice(b),
                StreamChunk::Kitty(k) => kitty.push(k),
            });
        }
        assert_eq!(normal, b"beforemiddleafter\x1b[31mred");
        assert_eq!(kitty, vec![b"a=q,i=7;AAAA".to_vec(), b"a=d,d=a".to_vec()]);
    }

    #[test]
    fn query_and_chunked_png_transmit_and_display_and_delete() {
        let mut store = make_store();
        let replies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let r2 = replies.clone();
        let reply = move |b: Vec<u8>| r2.lock().unwrap().push(String::from_utf8_lossy(&b).into_owned());

        // 1. Query support (`a=q`, 1x1 RGB)
        let mut inject = Vec::new();
        store.on_apc(b"i=31,s=1,v=1,a=q,t=d,f=24;AAAA", &mut inject, &reply);
        assert!(inject.is_empty());
        assert_eq!(replies.lock().unwrap().as_slice(), ["\x1b_Gi=31;OK\x1b\\"]);
        replies.lock().unwrap().clear();

        // 2. Chunked PNG transmit + display (`a=T,f=100,c=4,r=2`)
        let b64 = png_b64(16, 16);
        let mid = b64.len() / 2;
        let first = format!("a=T,f=100,i=5,c=4,r=2,m=1;{}", &b64[..mid]);
        let second = format!("m=0;{}", &b64[mid..]);
        assert!(!store.on_apc(first.as_bytes(), &mut inject, &reply));
        assert!(inject.is_empty());
        assert!(store.on_apc(second.as_bytes(), &mut inject, &reply));
        assert_eq!(replies.lock().unwrap().as_slice(), ["\x1b_Gi=5;OK\x1b\\"]);
        let injected = String::from_utf8(inject).unwrap();
        assert!(injected.contains("\x1b]2;knx-kg:1\x07\x1b[6n"));
        assert!(injected.contains("knx:img:1:0"));
        assert!(injected.contains("knx:img:1:1"));

        // 3. Resolve cursor & check visible image
        store.resolve_cursor(1, 0, 0);
        let mut anchors = HashMap::new();
        anchors.insert(1u64, (2i32, 3usize));
        let vis = store.visible_images(&anchors, false, 0, 0, 24);
        assert_eq!(vis.len(), 1);
        assert_eq!((vis[0].image_id, vis[0].line, vis[0].col, vis[0].cols, vis[0].rows), (5, 2, 3, 4, 2));
        assert_eq!(vis[0].data.rgba.get_pixel(0, 0).0, [255, 0, 128, 255]);

        // 4. Delete by id (`a=d,d=I,i=5`) frees image and placement
        let mut del_inject = Vec::new();
        assert!(store.on_apc(b"a=d,d=I,i=5", &mut del_inject, &reply));
        assert!(store.visible_images(&anchors, false, 0, 0, 24).is_empty());
        assert!(store.images.is_empty());
    }

    #[test]
    fn zlib_compressed_rgba_decodes() {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;

        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&[10, 20, 30, 255, 40, 50, 60, 255]).unwrap();
        let compressed = enc.finish().unwrap();
        let cmd = format!("a=t,f=32,s=2,v=1,o=z,i=9;{}", BASE64.encode(compressed));
        let (p, payload) = parse_command(cmd.as_bytes()).unwrap();
        let img = decode_kitty_image(&p, payload).unwrap();
        assert_eq!(img.dimensions(), (2, 1));
        assert_eq!(img.get_pixel(1, 0).0, [40, 50, 60, 255]);
    }

    #[test]
    fn anchored_placement_tracks_scrollback_in_real_term() {
        #[derive(Clone, Default)]
        struct ProbeListener {
            pending: Arc<Mutex<VecDeque<u64>>>,
            resolved: Arc<Mutex<Vec<(u64, usize, usize)>>>,
        }
        impl EventListener for ProbeListener {
            fn send_event(&self, event: Event) {
                match event {
                    Event::Title(t) => {
                        if let Some(seq) = t.strip_prefix(PROBE_PREFIX).and_then(|s| s.parse().ok()) {
                            self.pending.lock().unwrap().push_back(seq);
                        }
                    }
                    Event::PtyWrite(text) => {
                        if let Some((l, c)) = parse_cpr(&text)
                            && let Some(seq) = self.pending.lock().unwrap().pop_front()
                        {
                            self.resolved.lock().unwrap().push((seq, l, c));
                        }
                    }
                    _ => {}
                }
            }
        }

        let listener = ProbeListener::default();
        let mut term = Term::new(Config::default(), &Size { columns: 20, lines: 5 }, listener.clone());
        let mut parser: Processor = Processor::new();
        let mut store = make_store();
        let mut inject = Vec::new();
        let cmd = format!("a=T,f=100,i=1,c=4,r=3;{}", png_b64(16, 16));
        store.on_apc(cmd.as_bytes(), &mut inject, &|_| {});

        // Move cursor to row 4 (0-based 3), col 2, then place a 3-row image (triggers scroll!).
        let mut stream = b"\x1b[4;3H".to_vec();
        stream.extend_from_slice(&inject);
        stream.extend_from_slice(b"\r\nafter");
        parser.advance(&mut term, &stream);

        let resolved = listener.resolved.lock().unwrap();
        assert_eq!(resolved.as_slice(), [(1, 3, 2)]);

        // Verify the anchor cells landed in column 2 across 3 consecutive rows!
        let grid = term.grid();
        let mut found = Vec::new();
        for l in -(grid.history_size() as i32)..5 {
            if let Some(h) = grid[Line(l)][Column(2)].hyperlink()
                && let Some((key, row_i)) = parse_anchor_uri(h.uri())
            {
                found.push((l, key, row_i));
            }
        }
        assert_eq!(found.len(), 3);
        assert_eq!((found[0].1, found[0].2), (1, 0));
        assert_eq!((found[1].1, found[1].2), (1, 1));
        assert_eq!((found[2].1, found[2].2), (1, 2));
        assert_eq!(found[1].0, found[0].0 + 1);
        assert_eq!(found[2].0, found[1].0 + 1);
    }

    #[test]
    fn overlay_c1_placement_does_not_move_cursor_and_clears_on_alt_exit() {
        let mut store = make_store();
        store.last_alt_screen = true;
        let mut inject = Vec::new();
        let cmd = format!("a=T,f=100,i=2,c=6,r=4,C=1;{}", png_b64(16, 16));
        assert!(store.on_apc(cmd.as_bytes(), &mut inject, &|_| {}));
        assert_eq!(String::from_utf8(inject).unwrap(), "\x1b]2;knx-kg:1\x07\x1b[6n");
        store.resolve_cursor(1, 2, 5);

        let empty = HashMap::new();
        let vis = store.visible_images(&empty, true, 0, 0, 24);
        assert_eq!(vis.len(), 1);
        assert_eq!((vis[0].line, vis[0].col, vis[0].cols, vis[0].rows), (2, 5, 6, 4));
        // Leaving alt-screen clears alt-screen overlay placements.
        assert!(store.visible_images(&empty, false, 0, 0, 24).is_empty());
        assert!(store.visible_images(&empty, true, 0, 0, 24).is_empty());
    }
}
