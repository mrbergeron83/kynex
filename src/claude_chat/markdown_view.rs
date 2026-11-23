//! Renders `kynex_core::claude::markdown::Block`s in IBM Plex Sans / JetBrains Mono
//! with syntax-highlighted code blocks (`kynex_core::highlight`), Copy and `▶ Run`
//! shell buttons, clickable `file:line` links, external URLs, GFM tables, lists,
//! blockquotes, and inline images.

use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    AnyElement, ClipboardItem, Context, FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle,
    StyledText, UnderlineStyle, div, prelude::*,
};
use kynex_core::claude::diff::{DiffLineKind, EditDiff};
use kynex_core::claude::markdown::{Alignment, Block, Inline};
use kynex_core::highlight::{self, Lang, Side};

use super::ClaudeChatView;
use super::state::ClaudeChatEvent;
use crate::theme::{Palette, mono, theme, ui_font};
use crate::ui::{Icon, Tooltip, code as ui_code, ghost_button, icon, px, scrollbar};

/// Renders a slice of parsed Markdown blocks for row `row_id`.
pub(super) fn render_blocks(
    blocks: &[Block],
    row_id: u64,
    copied_key: Option<&str>,
    cx: &mut Context<ClaudeChatView>,
) -> Vec<AnyElement> {
    let t = theme(cx).clone();
    blocks.iter().enumerate().map(|(i, b)| render_block(b, &format!("r{row_id}-b{i}"), copied_key, &t, cx)).collect()
}

fn render_block(
    block: &Block,
    id_prefix: &str,
    copied_key: Option<&str>,
    t: &Palette,
    cx: &mut Context<ClaudeChatView>,
) -> AnyElement {
    match block {
        Block::Heading { level, inlines } => {
            let (size, weight, mt) = match level {
                1 => (px(18.), FontWeight::BOLD, px(8.)),
                2 => (px(15.5), FontWeight::SEMIBOLD, px(6.)),
                3 => (px(14.), FontWeight::SEMIBOLD, px(4.)),
                _ => (px(13.), FontWeight::SEMIBOLD, px(2.)),
            };
            div()
                .mt(mt)
                .font_family(ui_font())
                .font_weight(weight)
                .text_size(size)
                .text_color(t.text)
                .child(render_inlines(inlines, id_prefix, t, cx))
                .into_any_element()
        }
        Block::Paragraph(inlines) => div()
            .font_family(ui_font())
            .text_size(px(13.5))
            .line_height(px(21.))
            .text_color(t.text)
            .child(render_inlines(inlines, id_prefix, t, cx))
            .into_any_element(),
        Block::CodeBlock { lang, code } => render_code_block(lang.as_deref(), code, id_prefix, copied_key, t, cx),
        Block::BlockQuote(inner) => {
            let children: Vec<AnyElement> = inner
                .iter()
                .enumerate()
                .map(|(i, b)| render_block(b, &format!("{id_prefix}-q{i}"), copied_key, t, cx))
                .collect();
            div()
                .pl_3()
                .py_0p5()
                .border_l_2()
                .border_color(t.border_focused)
                .text_color(t.text_muted)
                .flex()
                .flex_col()
                .gap_1()
                .children(children)
                .into_any_element()
        }
        Block::List { ordered, start, items } => {
            let rows: Vec<AnyElement> = items
                .iter()
                .enumerate()
                .map(|(idx, item)| {
                    let marker = match item.checked {
                        Some(true) => div()
                            .flex_none()
                            .mt(px(3.))
                            .size(px(13.))
                            .rounded_sm()
                            .bg(t.accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(Icon::Check, t.on_accent).size(px(10.)))
                            .into_any_element(),
                        Some(false) => div()
                            .flex_none()
                            .mt(px(3.))
                            .size(px(13.))
                            .rounded_sm()
                            .border_1()
                            .border_color(t.border)
                            .into_any_element(),
                        None if *ordered => div()
                            .flex_none()
                            .min_w(px(18.))
                            .text_size(px(12.5))
                            .text_color(t.text_muted)
                            .child(format!("{}.", start + idx as u64))
                            .into_any_element(),
                        None => div()
                            .flex_none()
                            .w(px(12.))
                            .text_size(px(13.))
                            .text_color(t.text_muted)
                            .child("•")
                            .into_any_element(),
                    };
                    let body: Vec<AnyElement> = item
                        .blocks
                        .iter()
                        .enumerate()
                        .map(|(bi, b)| render_block(b, &format!("{id_prefix}-li{idx}-{bi}"), copied_key, t, cx))
                        .collect();
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(marker)
                        .child(div().flex_1().min_w_0().flex().flex_col().gap_1().children(body))
                        .into_any_element()
                })
                .collect();
            div().pl_1().flex().flex_col().gap_1().children(rows).into_any_element()
        }
        Block::Table { headers, alignments, rows } => render_table(headers, alignments, rows, id_prefix, t, cx),
        Block::Rule => div().my_1().h(px(1.)).w_full().bg(t.border_variant).into_any_element(),
        Block::Image { alt, src } => {
            let label = if alt.is_empty() { src.clone() } else { alt.clone() };
            let src_open = src.clone();
            let el_id: SharedString = format!("{id_prefix}-img").into();
            div()
                .id(el_id)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(t.border_variant)
                .bg(t.element)
                .text_xs()
                .text_color(t.accent)
                .cursor_pointer()
                .hover(|s| s.border_color(t.border_focused))
                .child(icon(Icon::File, t.accent).size(px(12.)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if src_open.starts_with("http://") || src_open.starts_with("https://") {
                        cx.open_url(&src_open);
                    } else {
                        let path = resolve_workspace_path(&this.config.workspace, &src_open);
                        this.open_image_file_in_lightbox(path, cx);
                    }
                }))
                .into_any_element()
        }
    }
}

fn render_table(
    headers: &[Vec<Inline>],
    alignments: &[Alignment],
    rows: &[Vec<Vec<Inline>>],
    id_prefix: &str,
    t: &Palette,
    cx: &mut Context<ClaudeChatView>,
) -> AnyElement {
    let header_cells: Vec<AnyElement> = headers
        .iter()
        .enumerate()
        .map(|(ci, cell)| {
            let align = alignments.get(ci).copied().unwrap_or(Alignment::None);
            let mut d = div()
                .flex_1()
                .min_w(px(64.))
                .px_2()
                .py_1()
                .font_weight(FontWeight::SEMIBOLD)
                .text_xs()
                .text_color(t.text);
            d = apply_align(d, align);
            d.child(render_inlines(cell, &format!("{id_prefix}-th{ci}"), t, cx)).into_any_element()
        })
        .collect();

    let body_rows: Vec<AnyElement> = rows
        .iter()
        .enumerate()
        .map(|(ri, row)| {
            let cells: Vec<AnyElement> = row
                .iter()
                .enumerate()
                .map(|(ci, cell)| {
                    let align = alignments.get(ci).copied().unwrap_or(Alignment::None);
                    let mut d = div().flex_1().min_w(px(64.)).px_2().py_1().text_xs().text_color(t.text);
                    d = apply_align(d, align);
                    d.child(render_inlines(cell, &format!("{id_prefix}-td{ri}-{ci}"), t, cx)).into_any_element()
                })
                .collect();
            div().flex().w_full().border_t_1().border_color(t.border_variant).children(cells).into_any_element()
        })
        .collect();

    div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(t.border_variant)
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(div().flex().w_full().bg(t.element).children(header_cells))
        .children(body_rows)
        .into_any_element()
}

fn apply_align(d: gpui::Div, align: Alignment) -> gpui::Div {
    match align {
        Alignment::Center => d.flex().justify_center(),
        Alignment::Right => d.flex().justify_end(),
        Alignment::Left | Alignment::None => d,
    }
}

fn render_code_block(
    lang: Option<&str>,
    code: &str,
    id_prefix: &str,
    copied_key: Option<&str>,
    t: &Palette,
    cx: &mut Context<ClaudeChatView>,
) -> AnyElement {
    let lang_label = lang.unwrap_or("text").to_string();
    let is_shell = lang.is_some_and(is_shell_lang);
    let hl_lang = lang.and_then(lang_from_fence);
    let copied = copied_key == Some(id_prefix);

    let copy_text = code.to_string();
    let copy_id_key = id_prefix.to_string();
    let copy_btn_id: SharedString = format!("{id_prefix}-copy").into();
    let run_btn_id: SharedString = format!("{id_prefix}-run").into();
    let run_code = code.to_string();

    let styled_code = if let Some(l) = hl_lang {
        let tokens = highlight::highlight(l, code);
        let styles = ui_code::styles(&tokens, &t.syntax);
        StyledText::new(code.to_string()).with_highlights(styles)
    } else {
        StyledText::new(code.to_string())
    };

    div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(t.border_variant)
        .bg(t.element)
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .h(px(26.))
                .px_2p5()
                .border_b_1()
                .border_color(t.border_variant)
                .bg(t.panel)
                .child(div().font_family(mono()).text_size(px(11.)).text_color(t.text_muted).child(lang_label))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(is_shell, |d| {
                            d.child(
                                ghost_button(run_btn_id, cx)
                                    .h(px(20.))
                                    .px_1p5()
                                    .text_xs()
                                    .text_color(t.accent)
                                    .tooltip(Tooltip::text("Run in workspace terminal"))
                                    .child("▶ Run")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ClaudeChatEvent::RunInTerminal(run_code.clone()));
                                    })),
                            )
                        })
                        .child(
                            ghost_button(copy_btn_id, cx)
                                .h(px(20.))
                                .px_1p5()
                                .text_xs()
                                .text_color(if copied { t.success } else { t.text_muted })
                                .tooltip(Tooltip::text("Copy code"))
                                .child(
                                    icon(
                                        if copied { Icon::Check } else { Icon::Copy },
                                        if copied { t.success } else { t.text_muted },
                                    )
                                    .size(px(11.)),
                                )
                                .child(if copied { "Copied" } else { "Copy" })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                                    this.mark_copied(copy_id_key.clone(), cx);
                                })),
                        ),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id_prefix}-code-scroll")))
                .w_full()
                .overflow_x_scroll()
                .px_3()
                .py_2()
                .font_family(mono())
                .text_size(px(12.2))
                .line_height(px(18.5))
                .text_color(t.text)
                .child(styled_code),
        )
        .into_any_element()
}

/// Renders inline Markdown nodes: uses fast `StyledText` for pure-text runs, and
/// splits interactive `Link` / `FileRef` tokens into clickable inline elements.
fn render_inlines(inlines: &[Inline], id_prefix: &str, t: &Palette, cx: &mut Context<ClaudeChatView>) -> AnyElement {
    if !has_interactive_inline(inlines) {
        let mut text = String::new();
        let mut spans: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        flatten_styled(inlines, &mut text, &mut spans, StyleFlags::default(), t);
        if spans.is_empty() {
            return div().child(text).into_any_element();
        }
        return StyledText::new(text).with_highlights(spans).into_any_element();
    }

    // Mix of styled text segments and clickable links / file:line references.
    let mut pieces: Vec<Piece> = Vec::new();
    collect_pieces(inlines, StyleFlags::default(), &mut pieces);

    let mut elements: Vec<AnyElement> = Vec::new();
    let mut buf_text = String::new();
    let mut buf_spans: Vec<(Range<usize>, HighlightStyle)> = Vec::new();

    let flush =
        |buf_text: &mut String, buf_spans: &mut Vec<(Range<usize>, HighlightStyle)>, elements: &mut Vec<AnyElement>| {
            if buf_text.is_empty() {
                return;
            }
            let s = std::mem::take(buf_text);
            let sp = std::mem::take(buf_spans);
            if sp.is_empty() {
                elements.push(div().child(s).into_any_element());
            } else {
                elements.push(StyledText::new(s).with_highlights(sp).into_any_element());
            }
        };

    for (idx, piece) in pieces.into_iter().enumerate() {
        match piece {
            Piece::Run { text, flags } => {
                let start = buf_text.len();
                buf_text.push_str(&text);
                let end = buf_text.len();
                if let Some(style) = flags.to_style(t) {
                    buf_spans.push((start..end, style));
                }
            }
            Piece::Link { label, url } => {
                flush(&mut buf_text, &mut buf_spans, &mut elements);
                let el_id: SharedString = format!("{id_prefix}-lnk-{idx}").into();
                let url_click = url.clone();
                elements.push(
                    div()
                        .id(el_id)
                        .text_color(t.accent)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .tooltip(Tooltip::text(url))
                        .child(label)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.open_url(&url_click);
                        }))
                        .into_any_element(),
                );
            }
            Piece::FileRef { raw, path, line } => {
                flush(&mut buf_text, &mut buf_spans, &mut elements);
                let el_id: SharedString = format!("{id_prefix}-fref-{idx}").into();
                let path_click = path.clone();
                elements.push(
                    div()
                        .id(el_id)
                        .flex()
                        .items_center()
                        .px_1()
                        .rounded_sm()
                        .bg(t.element)
                        .font_family(mono())
                        .text_size(px(12.))
                        .text_color(t.accent)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).underline())
                        .tooltip(Tooltip::text(format!("Open {raw}")))
                        .child(raw)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let full = resolve_workspace_path(&this.config.workspace, &path_click);
                            cx.emit(ClaudeChatEvent::OpenFile { path: full, line });
                        }))
                        .into_any_element(),
                );
            }
        }
    }
    flush(&mut buf_text, &mut buf_spans, &mut elements);

    div().flex().flex_wrap().items_baseline().gap_x(px(2.)).children(elements).into_any_element()
}

#[derive(Clone, Copy, Default)]
struct StyleFlags {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
}

impl StyleFlags {
    fn to_style(self, t: &Palette) -> Option<HighlightStyle> {
        if !self.bold && !self.italic && !self.strike && !self.code {
            return None;
        }
        Some(HighlightStyle {
            color: self.code.then_some(t.accent),
            font_weight: self.bold.then_some(FontWeight::SEMIBOLD),
            font_style: self.italic.then_some(FontStyle::Italic),
            background_color: self.code.then_some(t.element),
            strikethrough: self.strike.then_some(StrikethroughStyle { thickness: gpui::px(1.), color: None }),
            underline: None::<UnderlineStyle>,
            ..Default::default()
        })
    }
}

enum Piece {
    Run { text: String, flags: StyleFlags },
    Link { label: String, url: String },
    FileRef { raw: String, path: String, line: Option<u32> },
}

fn has_interactive_inline(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inl| match inl {
        Inline::Link { .. } | Inline::FileRef { .. } => true,
        Inline::Bold(v) | Inline::Italic(v) | Inline::Strike(v) => has_interactive_inline(v),
        Inline::Text(_) | Inline::Code(_) => false,
    })
}

fn flatten_styled(
    inlines: &[Inline],
    out: &mut String,
    spans: &mut Vec<(Range<usize>, HighlightStyle)>,
    flags: StyleFlags,
    t: &Palette,
) {
    for inl in inlines {
        match inl {
            Inline::Text(s) => {
                let start = out.len();
                out.push_str(s);
                if let Some(style) = flags.to_style(t) {
                    spans.push((start..out.len(), style));
                }
            }
            Inline::Code(s) => {
                let start = out.len();
                out.push_str(s);
                let mut f = flags;
                f.code = true;
                if let Some(style) = f.to_style(t) {
                    spans.push((start..out.len(), style));
                }
            }
            Inline::Bold(inner) => {
                let mut f = flags;
                f.bold = true;
                flatten_styled(inner, out, spans, f, t);
            }
            Inline::Italic(inner) => {
                let mut f = flags;
                f.italic = true;
                flatten_styled(inner, out, spans, f, t);
            }
            Inline::Strike(inner) => {
                let mut f = flags;
                f.strike = true;
                flatten_styled(inner, out, spans, f, t);
            }
            Inline::Link { label, .. } => out.push_str(label),
            Inline::FileRef { raw, .. } => out.push_str(raw),
        }
    }
}

fn collect_pieces(inlines: &[Inline], flags: StyleFlags, out: &mut Vec<Piece>) {
    for inl in inlines {
        match inl {
            Inline::Text(s) => out.push(Piece::Run { text: s.clone(), flags }),
            Inline::Code(s) => {
                let mut f = flags;
                f.code = true;
                out.push(Piece::Run { text: s.clone(), flags: f });
            }
            Inline::Bold(inner) => {
                let mut f = flags;
                f.bold = true;
                collect_pieces(inner, f, out);
            }
            Inline::Italic(inner) => {
                let mut f = flags;
                f.italic = true;
                collect_pieces(inner, f, out);
            }
            Inline::Strike(inner) => {
                let mut f = flags;
                f.strike = true;
                collect_pieces(inner, f, out);
            }
            Inline::Link { label, url } => out.push(Piece::Link { label: label.clone(), url: url.clone() }),
            Inline::FileRef { raw, path, line } => {
                out.push(Piece::FileRef { raw: raw.clone(), path: path.clone(), line: *line });
            }
        }
    }
}

/// Resolves a possibly-relative path against `workspace`.
pub(super) fn resolve_workspace_path(workspace: &std::path::Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { workspace.join(p) }
}

/// Returns `true` for shell / command fenced code blocks that get a `▶ Run` button.
pub(super) fn is_shell_lang(lang: &str) -> bool {
    matches!(
        lang.trim().to_ascii_lowercase().as_str(),
        "sh" | "bash" | "zsh" | "shell" | "console" | "powershell" | "ps1" | "pwsh" | "cmd" | "bat"
    )
}

/// Maps a Markdown code fence language tag to `kynex_core::highlight::Lang`.
pub(super) fn lang_from_fence(lang: &str) -> Option<Lang> {
    let ext = match lang.trim().to_ascii_lowercase().as_str() {
        "rs" | "rust" => "rs",
        "ts" | "typescript" => "ts",
        "tsx" => "tsx",
        "js" | "javascript" | "mjs" | "cjs" => "js",
        "jsx" => "jsx",
        "py" | "python" => "py",
        "go" | "golang" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "c++" | "hpp" => "cpp",
        "cs" | "csharp" => "cs",
        "java" => "java",
        "kt" | "kotlin" => "kt",
        "swift" => "swift",
        "rb" | "ruby" => "rb",
        "php" => "php",
        "sql" => "sql",
        "sh" | "bash" | "zsh" | "shell" => "sh",
        "ps1" | "pwsh" | "powershell" => "ps1",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "html" | "htm" | "xml" | "svg" => "html",
        "css" | "scss" => "css",
        _ => return None,
    };
    Lang::from_path(&format!("file.{ext}"))
}

/// Renders a compact syntax-colored inline unified diff for an expanded `EditRow` or permission preview.
pub(super) fn render_inline_diff(
    path: &str,
    diff: &EditDiff,
    error_text: Option<&str>,
    id: u64,
    t: &Palette,
) -> AnyElement {
    let lang = Lang::from_path(path);
    let mut rows: Vec<AnyElement> = Vec::new();

    if let Some(err) = error_text {
        rows.push(
            div()
                .px_2p5()
                .py_1()
                .bg(t.diff_del)
                .font_family(mono())
                .text_xs()
                .text_color(t.error)
                .child(err.to_string())
                .into_any_element(),
        );
    }

    for hunk in &diff.hunks {
        rows.push(
            div()
                .px_2p5()
                .py_0p5()
                .bg(t.panel)
                .font_family(mono())
                .text_size(px(11.))
                .text_color(t.text_placeholder)
                .child(format!("@@ -{},{} +{},{} @@", hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count))
                .into_any_element(),
        );
        let texts: Vec<String> = hunk.lines.iter().map(|l| l.text.replace('\t', "    ")).collect();
        let mut tokens = lang
            .map(|l| {
                let sides = hunk.lines.iter().zip(&texts).map(|(line, txt)| {
                    let s = match line.kind {
                        DiffLineKind::Remove => Side::Old,
                        DiffLineKind::Add => Side::New,
                        DiffLineKind::Context => Side::Both,
                    };
                    (s, txt.as_str())
                });
                highlight::diff(l, sides)
            })
            .unwrap_or_default()
            .into_iter();

        for (line, text) in hunk.lines.iter().zip(texts) {
            let line_tokens = tokens.next().unwrap_or_default();
            let (bg, sign, sign_color) = match line.kind {
                DiffLineKind::Add => (Some(t.diff_add), "+", t.success),
                DiffLineKind::Remove => (Some(t.diff_del), "-", t.error),
                DiffLineKind::Context => (None, " ", t.text_placeholder),
            };
            let old_no = line.old_line.map_or_else(String::new, |n| n.to_string());
            let new_no = line.new_line.map_or_else(String::new, |n| n.to_string());
            let code_el: AnyElement = if line_tokens.is_empty() {
                div().child(text).into_any_element()
            } else {
                StyledText::new(text).with_highlights(ui_code::styles(&line_tokens, &t.syntax)).into_any_element()
            };
            rows.push(
                div()
                    .w_full()
                    .flex()
                    .items_baseline()
                    .when_some(bg, |d, c| d.bg(c))
                    .font_family(mono())
                    .text_size(px(11.5))
                    .line_height(px(18.))
                    .child(
                        div().w(px(34.)).flex_none().pr_1().text_right().text_color(t.text_placeholder).child(old_no),
                    )
                    .child(
                        div().w(px(34.)).flex_none().pr_1p5().text_right().text_color(t.text_placeholder).child(new_no),
                    )
                    .child(div().w(px(14.)).flex_none().text_color(sign_color).child(sign))
                    .child(div().flex_1().min_w_0().pr_2().text_color(t.text).child(code_el))
                    .into_any_element(),
            );
        }
    }

    if diff.truncated {
        rows.push(
            div()
                .px_2p5()
                .py_1()
                .bg(t.panel)
                .text_xs()
                .text_color(t.text_muted)
                .child("Diff truncated — inspect full changes in the Git pane.")
                .into_any_element(),
        );
    }
    if rows.is_empty() {
        rows.push(
            div()
                .px_2p5()
                .py_1p5()
                .text_xs()
                .text_color(t.text_placeholder)
                .child("No line changes.")
                .into_any_element(),
        );
    }

    let scroll = scrollbar::handle(("edit-diff-sb", id));
    scrollbar::vertical_over(
        ("edit-diff-sbar", id),
        scroll.clone(),
        div()
            .id(SharedString::from(format!("edit-diff-{id}")))
            .max_h(px(300.))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .border_t_1()
            .border_color(t.border_variant)
            .bg(t.editor)
            .flex()
            .flex_col()
            .children(rows),
        |wrap| wrap.w_full(),
    )
    .into_any_element()
}
