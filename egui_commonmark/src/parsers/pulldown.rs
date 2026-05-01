use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::iter::Peekable;
use std::ops::Range;

use crate::{CommonMarkCache, CommonMarkOptions};

use egui::{self, Id, Pos2, TextStyle, Ui};

use crate::List;
use egui_commonmark_backend::elements::*;
use egui_commonmark_backend::misc::*;
use egui_commonmark_backend::pulldown::*;
use pulldown_cmark::{CowStr, HeadingLevel};

/// Newline logic is constructed by the following:
/// All elements try to insert a newline before them (if they are allowed)
/// and end their own line.
struct Newline {
    /// Whether a newline should not be inserted before a widget. This is only for
    /// the first widget.
    should_not_start_newline_forced: bool,
    /// Whether an element should insert a newline before it
    should_start_newline: bool,
    /// Whether an element should end it's own line using a newline
    /// This will have to be set to false in cases such as when blocks are within
    /// a list.
    should_end_newline: bool,
    /// only false when the widget is the last one.
    should_end_newline_forced: bool,
}

impl Default for Newline {
    fn default() -> Self {
        Self {
            should_not_start_newline_forced: true,
            should_start_newline: true,
            should_end_newline: true,
            should_end_newline_forced: true,
        }
    }
}

impl Newline {
    pub fn can_insert_end(&self) -> bool {
        self.should_end_newline && self.should_end_newline_forced
    }

    pub fn can_insert_start(&self) -> bool {
        self.should_start_newline && !self.should_not_start_newline_forced
    }

    pub fn try_insert_start(&self, ui: &mut Ui) {
        if self.can_insert_start() {
            newline(ui);
        }
    }

    pub fn try_insert_end(&self, ui: &mut Ui) {
        if self.can_insert_end() {
            newline(ui);
        }
    }
}

#[derive(Default)]
struct DefinitionList {
    is_first_item: bool,
    is_def_list_def: bool,
}

pub struct CommonMarkViewerInternal {
    curr_table: usize,
    text_style: Style,
    list: List,
    link: Option<Link>,
    image: Option<Image>,
    line: Newline,
    code_block: Option<CodeBlock>,

    /// Only populated if the html_fn option has been set
    html_block: String,
    is_list_item: bool,
    def_list: DefinitionList,
    is_table: bool,
    is_blockquote: bool,
    checkbox_events: Vec<CheckboxClickEvent>,
}

pub(crate) struct CheckboxClickEvent {
    pub(crate) checked: bool,
    pub(crate) span: Range<usize>,
}

impl CommonMarkViewerInternal {
    pub fn new() -> Self {
        Self {
            curr_table: 0,
            text_style: Style::default(),
            list: List::default(),
            link: None,
            image: None,
            line: Newline::default(),
            is_list_item: false,
            def_list: Default::default(),
            code_block: None,
            html_block: String::new(),
            is_table: false,
            is_blockquote: false,
            checkbox_events: Vec::new(),
        }
    }
}

fn parser_options_math(is_math_enabled: bool) -> pulldown_cmark::Options {
    if is_math_enabled {
        parser_options() | pulldown_cmark::Options::ENABLE_MATH
    } else {
        parser_options()
    }
}

impl CommonMarkViewerInternal {
    /// Be aware that this acquires egui::Context internally.
    /// If split Id is provided then split points will be populated
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        text: &str,
        split_points_id: Option<Id>,
    ) -> (egui::InnerResponse<()>, Vec<CheckboxClickEvent>) {
        let max_width = options.max_width(ui);
        let layout = egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true);

        let re = ui.allocate_ui_with_layout(egui::vec2(max_width, 0.0), layout, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let height = ui.text_style_height(&TextStyle::Body);
            ui.set_row_height(height);

            let mut events = pulldown_cmark::Parser::new_ext(
                text,
                parser_options_math(options.math_fn.is_some()),
            )
            .into_offset_iter()
            .enumerate()
            .peekable();

            while let Some((index, (e, src_span))) = events.next() {
                let start_position = ui.next_widget_position();
                let is_element_end = matches!(e, pulldown_cmark::Event::End(_));
                let should_add_split_point = self.list.is_inside_a_list() && is_element_end;

                if events.peek().is_none() {
                    self.line.should_end_newline_forced = false;
                }

                self.process_event(ui, &mut events, e, src_span, cache, options, max_width);

                if let Some(source_id) = split_points_id {
                    if should_add_split_point {
                        let scroll_cache = scroll_cache(cache, &source_id);
                        let end_position = ui.next_widget_position();

                        let split_point_exists = scroll_cache
                            .split_points
                            .iter()
                            .any(|(i, _, _)| *i == index);

                        if !split_point_exists {
                            scroll_cache
                                .split_points
                                .push((index, start_position, end_position));
                        }
                    }
                }

                if index == 0 {
                    self.line.should_not_start_newline_forced = false;
                }
            }

            if let Some(source_id) = split_points_id {
                scroll_cache(cache, &source_id).page_size =
                    Some(ui.next_widget_position().to_vec2());
            }
        });

        (re, std::mem::take(&mut self.checkbox_events))
    }

    pub(crate) fn show_scrollable(
        &mut self,
        source_id: Id,
        ui: &mut egui::Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        text: &str,
    ) {
        let available_size = ui.available_size();
        let scroll_id = source_id.with("_scroll_area");

        let Some(page_size) = scroll_cache(cache, &source_id).page_size else {
            egui::ScrollArea::vertical()
                .id_salt(scroll_id)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    self.show(ui, cache, options, text, Some(source_id));
                });
            // Prevent repopulating points twice at startup
            scroll_cache(cache, &source_id).available_size = available_size;
            return;
        };

        let events =
            pulldown_cmark::Parser::new_ext(text, parser_options_math(options.math_fn.is_some()))
                .into_offset_iter()
                .collect::<Vec<_>>();

        let num_rows = events.len();

        egui::ScrollArea::vertical()
            .id_salt(scroll_id)
            // Elements have different widths, so the scroll area cannot try to shrink to the
            // content, as that will mean that the scroll bar will move when loading elements
            // with different widths.
            .auto_shrink([false, true])
            .show_viewport(ui, |ui, viewport| {
                ui.set_height(page_size.y);
                let layout = egui::Layout::left_to_right(egui::Align::BOTTOM).with_main_wrap(true);

                let max_width = options.max_width(ui);
                ui.allocate_ui_with_layout(egui::vec2(max_width, 0.0), layout, |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let scroll_cache = scroll_cache(cache, &source_id);

                    // finding the first element that's not in the viewport anymore
                    let (first_event_index, _, first_end_position) = scroll_cache
                        .split_points
                        .iter()
                        .filter(|(_, _, end_position)| end_position.y < viewport.min.y)
                        .nth_back(1)
                        .copied()
                        .unwrap_or((0, Pos2::ZERO, Pos2::ZERO));

                    // finding the last element that's just outside the viewport
                    let last_event_index = scroll_cache
                        .split_points
                        .iter()
                        .filter(|(_, start_position, _)| start_position.y > viewport.max.y)
                        .nth(1)
                        .map(|(index, _, _)| *index)
                        .unwrap_or(num_rows);

                    ui.allocate_space(first_end_position.to_vec2());

                    // only rendering the elements that are inside the viewport
                    let mut events = events
                        .into_iter()
                        .enumerate()
                        .skip(first_event_index)
                        .take(last_event_index - first_event_index)
                        .peekable();

                    while let Some((i, (e, src_span))) = events.next() {
                        if events.peek().is_none() {
                            self.line.should_end_newline_forced = false;
                        }

                        self.process_event(ui, &mut events, e, src_span, cache, options, max_width);

                        if i == 0 {
                            self.line.should_not_start_newline_forced = false;
                        }
                    }
                });
            });

        // Forcing full re-render to repopulate split points for the new size
        let scroll_cache = scroll_cache(cache, &source_id);
        if available_size != scroll_cache.available_size {
            scroll_cache.available_size = available_size;
            scroll_cache.page_size = None;
            scroll_cache.split_points.clear();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_event<'e>(
        &mut self,
        ui: &mut Ui,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        event: pulldown_cmark::Event,
        src_span: Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        self.event(ui, event, src_span, cache, options, max_width);

        self.def_list_def_wrapping(events, max_width, cache, options, ui);
        self.item_list_wrapping(events, max_width, cache, options, ui);
        self.table(events, cache, options, ui, max_width);
        self.blockquote(events, max_width, cache, options, ui);
    }

    fn def_list_def_wrapping<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.def_list.is_def_list_def {
            self.def_list.is_def_list_def = false;

            let item_events = delayed_events(events, |tag| {
                matches!(tag, pulldown_cmark::TagEnd::DefinitionListDefinition)
            });

            let mut events_iter = item_events.into_iter().enumerate().peekable();

            self.line.try_insert_start(ui);

            // Proccess a single event separately so that we do not insert spaces where we do not
            // want them
            self.line.should_start_newline = false;
            if let Some((_, (e, src_span))) = events_iter.next() {
                self.process_event(ui, &mut events_iter, e, src_span, cache, options, max_width);
            }

            ui.label(" ".repeat(options.indentation_spaces));
            self.line.should_start_newline = true;
            self.line.should_end_newline = false;
            // Required to ensure that the content is aligned with the identation
            ui.horizontal_wrapped(|ui| {
                while let Some((_, (e, src_span))) = events_iter.next() {
                    self.process_event(
                        ui,
                        &mut events_iter,
                        e,
                        src_span,
                        cache,
                        options,
                        max_width,
                    );
                }
            });
            self.line.should_end_newline = true;

            // Only end the definition items line if it is not the last element in the list
            if !matches!(
                events.peek(),
                Some((
                    _,
                    (
                        pulldown_cmark::Event::End(pulldown_cmark::TagEnd::DefinitionList),
                        _
                    )
                ))
            ) {
                self.line.try_insert_end(ui);
            }
        }
    }

    fn item_list_wrapping<'e>(
        &mut self,
        events: &mut impl Iterator<Item = EventIteratorItem<'e>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.is_list_item {
            self.is_list_item = false;

            let item_events = delayed_events_list_item(events);
            let mut events_iter = item_events.into_iter().enumerate().peekable();

            // Required to ensure that the content of the list item is aligned with
            // the * or - when wrapping
            ui.horizontal_wrapped(|ui| {
                while let Some((_, (e, src_span))) = events_iter.next() {
                    self.process_event(
                        ui,
                        &mut events_iter,
                        e,
                        src_span,
                        cache,
                        options,
                        max_width,
                    );
                }
            });
        }
    }

    fn blockquote<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        max_width: f32,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
    ) {
        if self.is_blockquote {
            let mut collected_events = delayed_events(events, |tag| {
                matches!(tag, pulldown_cmark::TagEnd::BlockQuote(_))
            });
            self.line.try_insert_start(ui);

            // Currently the blockquotes are made in such a way that they need a newline at the end
            // and the start so when this is the first element in the markdown the newline must be
            // manually enabled
            self.line.should_not_start_newline_forced = false;
            if let Some(alert) = parse_alerts(&options.alerts, &mut collected_events) {
                egui_commonmark_backend::alert_ui(alert, ui, |ui| {
                    for (event, src_span) in collected_events {
                        self.event(ui, event, src_span, cache, options, max_width);
                    }
                })
            } else {
                blockquote(ui, ui.visuals().weak_text_color(), |ui| {
                    self.text_style.quote = true;
                    for (event, src_span) in collected_events {
                        self.event(ui, event, src_span, cache, options, max_width);
                    }
                    self.text_style.quote = false;
                });
            }

            if events.peek().is_none() {
                self.line.should_end_newline_forced = false;
            }

            self.line.try_insert_end(ui);
            self.is_blockquote = false;
        }
    }

    fn table<'e>(
        &mut self,
        events: &mut Peekable<impl Iterator<Item = EventIteratorItem<'e>>>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        ui: &mut Ui,
        max_width: f32,
    ) {
        if self.is_table {
            self.line.try_insert_start(ui);

            let id = ui.id().with("_table").with(self.curr_table);
            self.curr_table += 1;

            // We avoid `egui::Grid` here: it auto-sizes columns to widest
            // content (no per-column width control) and its row-height
            // plumbing did not propagate wrapped-cell heights reliably,
            // causing 3+ line cells to overlap the next row. Instead we
            // measure per-column content width, distribute the available
            // width proportionally, and lay out manually with
            // vertical → horizontal → wrapping per cell. `horizontal_wrapped`
            // inside a fixed-width child ui wraps text and reports the true
            // multi-line height up to the parent vertical layout.
            let _ = (id, max_width);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                let Table { mut header, mut rows } = parse_table(events);
                let num_cols = header.len().max(1);

                // Per GFM ("Spaces around cell content are ignored") strip
                // leading whitespace from the first text-bearing event and
                // trailing whitespace from the last. pulldown-cmark already
                // skips leading whitespace before parsing, but trailing
                // whitespace before the `|` delimiter survives into Text
                // events and would otherwise render as visible spaces.
                fn trim_text_event(e: &mut pulldown_cmark::Event<'_>, end: bool) -> bool {
                    let s = match e {
                        pulldown_cmark::Event::Text(s)
                        | pulldown_cmark::Event::Code(s)
                        | pulldown_cmark::Event::InlineHtml(s)
                        | pulldown_cmark::Event::Html(s) => s,
                        _ => return false,
                    };
                    let trimmed: String = if end {
                        s.trim_end().to_owned()
                    } else {
                        s.trim_start().to_owned()
                    };
                    if trimmed.len() != s.len() {
                        *s = pulldown_cmark::CowStr::Boxed(trimmed.into_boxed_str());
                    }
                    !s.as_ref().is_empty()
                }
                fn trim_cell(col: &mut [(pulldown_cmark::Event<'_>, Range<usize>)]) {
                    for (e, _) in col.iter_mut() {
                        if trim_text_event(e, false) {
                            break;
                        }
                    }
                    for (e, _) in col.iter_mut().rev() {
                        if trim_text_event(e, true) {
                            break;
                        }
                    }
                }
                for col in header.iter_mut() {
                    trim_cell(col);
                }
                for row in rows.iter_mut() {
                    for col in row.iter_mut() {
                        trim_cell(col);
                    }
                }

                // `Style::resolve_font_id` is the single source of truth for
                // mapping a Style snapshot → FontId, shared with the renderer
                // (`to_richtext_with`). The walker below tracks Style
                // transitions exactly the way `start_tag`/`end_tag` mutate
                // `self.text_style`, so each measured run picks the same
                // FontId the renderer will apply (honoring override_font_id,
                // monospace for code, and strong_font_family for **strong**).
                let ui_style = ui.style().clone();
                let strong_family = options.strong_font_family.as_ref();
                // Visible gap between adjacent cell contents. Applied on the
                // row layout's item_spacing.x so it's also accounted for in
                // the per-column width budget below.
                let spacing_x = ui.spacing().item_spacing.x.max(8.0);
                let avail = ui.available_width();
                let total_spacing = spacing_x * (num_cols.saturating_sub(1)) as f32;
                let min_col_floor = 24.0_f32;
                let usable = (avail - total_spacing).max(num_cols as f32 * min_col_floor);

                // Hash the table content to key the per-table measurement
                // cache. Recomputing layout-job widths for every cell every
                // frame would be wasteful — measurements only change when
                // the markdown source changes (typical case: streamed LLM
                // output), so we memoize on `CommonMarkCache`.
                let content_hash: u64 = {
                    let mut h = DefaultHasher::new();
                    num_cols.hash(&mut h);
                    // Hash both text content and style-tag transitions: the
                    // measurement cost (number of render-time labels and the
                    // spacing_x gaps between them) depends on Strong/Emphasis/
                    // Link boundaries and on Soft/Hard breaks, so cache
                    // invalidation must follow the same shape.
                    let hash_col =
                        |col: &[(pulldown_cmark::Event<'_>, Range<usize>)],
                         h: &mut DefaultHasher| {
                            for (e, _) in col {
                                match e {
                                    pulldown_cmark::Event::Code(s) => {
                                        1u8.hash(h);
                                        s.as_ref().hash(h);
                                    }
                                    pulldown_cmark::Event::Text(s)
                                    | pulldown_cmark::Event::InlineHtml(s)
                                    | pulldown_cmark::Event::Html(s) => {
                                        0u8.hash(h);
                                        s.as_ref().hash(h);
                                    }
                                    pulldown_cmark::Event::SoftBreak => {
                                        0x40u8.hash(h);
                                    }
                                    pulldown_cmark::Event::HardBreak => {
                                        0x41u8.hash(h);
                                    }
                                    pulldown_cmark::Event::Start(t) => match t {
                                        pulldown_cmark::Tag::Strong => {
                                            0x10u8.hash(h)
                                        }
                                        pulldown_cmark::Tag::Emphasis => {
                                            0x12u8.hash(h)
                                        }
                                        pulldown_cmark::Tag::Strikethrough => {
                                            0x14u8.hash(h)
                                        }
                                        pulldown_cmark::Tag::Link {
                                            dest_url, ..
                                        } => {
                                            0x20u8.hash(h);
                                            dest_url.as_ref().hash(h);
                                        }
                                        pulldown_cmark::Tag::Image {
                                            dest_url, ..
                                        } => {
                                            0x30u8.hash(h);
                                            dest_url.as_ref().hash(h);
                                        }
                                        _ => {}
                                    },
                                    pulldown_cmark::Event::End(t) => match t {
                                        pulldown_cmark::TagEnd::Strong => {
                                            0x11u8.hash(h)
                                        }
                                        pulldown_cmark::TagEnd::Emphasis => {
                                            0x13u8.hash(h)
                                        }
                                        pulldown_cmark::TagEnd::Strikethrough => {
                                            0x15u8.hash(h)
                                        }
                                        pulldown_cmark::TagEnd::Link => {
                                            0x21u8.hash(h)
                                        }
                                        pulldown_cmark::TagEnd::Image => {
                                            0x31u8.hash(h)
                                        }
                                        _ => {}
                                    },
                                    _ => {}
                                }
                            }
                            0xFFu8.hash(h);
                        };
                    for col in header.iter() {
                        hash_col(col, &mut h);
                    }
                    0xEEu8.hash(&mut h);
                    for row in rows.iter() {
                        for col in row.iter() {
                            hash_col(col, &mut h);
                        }
                        0xEEu8.hash(&mut h);
                    }
                    h.finish()
                };

                // Build / refresh the cached per-column metrics.
                {
                    let cached = table_cache(cache, &id);
                    if cached.content_hash != content_hash
                        || cached.col_natural.len() != num_cols
                    {
                        // Returns (natural_w, min_w) for one cell.
                        //
                        // The cell renderer at the bottom of this function
                        // dispatches every inline event through `self.event`,
                        // which emits one `ui.label` per Text/Code/InlineHtml/
                        // Html/SoftBreak event, one `ui.label("\n")` per
                        // HardBreak (row terminator), and one `ui.link` /
                        // `ui.hyperlink_to` per `[text](url)` link (the inner
                        // text runs are collapsed into a single LayoutJob and
                        // rendered as a single widget). Strong/Emphasis/
                        // Strikethrough are pure state mutations — no widget.
                        //
                        // The cell ui sets `item_spacing.x = spacing_x` (~8px),
                        // so adjacent widgets are separated by `spacing_x` at
                        // render time. Measuring multiple runs as one combined
                        // LayoutJob therefore under-reports the rendered width
                        // by `(K-1) * spacing_x` and forces the last run onto a
                        // new line whenever the column allocator gives a width
                        // close to the under-measured natural. This walk
                        // mirrors render exactly: per-widget intrinsic width
                        // plus inter-widget spacing.
                        //
                        // Each Text/Code/InlineHtml/Html/SoftBreak event
                        // becomes one widget at render time. We mirror the
                        // walk over events, snapshotting the running `Style`
                        // for each text run so the FontId we measure with is
                        // exactly the FontId the renderer resolves via
                        // `Style::to_richtext_with` → `Style::resolve_font_id`.
                        fn measure_cell(
                            f: &mut egui::epaint::text::FontsView<'_>,
                            col: &[(pulldown_cmark::Event<'_>, Range<usize>)],
                            ui_style: &egui::Style,
                            strong_family: Option<&egui::FontFamily>,
                            spacing_x: f32,
                        ) -> (f32, f32) {
                            let color = egui::Color32::WHITE;

                            #[derive(Clone)]
                            struct Run {
                                s: String,
                                style: Style,
                            }
                            #[derive(Clone)]
                            enum Widget {
                                Text(Run),
                                SoftBreak,
                                HardBreak,
                                Link(Vec<Run>),
                            }

                            let mut widgets: Vec<Widget> = Vec::new();
                            // Running Style mirrors `start_tag`/`end_tag`'s
                            // mutations of `self.text_style`. Strong/Emphasis/
                            // Strikethrough nest, but pulldown won't emit
                            // overlapping opens for the same kind, so booleans
                            // suffice (matching the renderer).
                            let mut style = Style::default();
                            let mut in_link = false;
                            let mut in_image = false;
                            let mut link_runs: Vec<Run> = Vec::new();

                            for (e, _) in col {
                                match e {
                                    pulldown_cmark::Event::Start(tag) => match tag {
                                        pulldown_cmark::Tag::Link { .. } => {
                                            in_link = true;
                                            link_runs.clear();
                                        }
                                        pulldown_cmark::Tag::Image { .. } => {
                                            in_image = true;
                                        }
                                        pulldown_cmark::Tag::Strong => {
                                            style.strong = true;
                                        }
                                        pulldown_cmark::Tag::Emphasis => {
                                            style.emphasis = true;
                                        }
                                        pulldown_cmark::Tag::Strikethrough => {
                                            style.strikethrough = true;
                                        }
                                        _ => {}
                                    },
                                    pulldown_cmark::Event::End(tag) => match tag {
                                        pulldown_cmark::TagEnd::Link => {
                                            widgets.push(Widget::Link(
                                                std::mem::take(&mut link_runs),
                                            ));
                                            in_link = false;
                                        }
                                        pulldown_cmark::TagEnd::Image => {
                                            in_image = false;
                                        }
                                        pulldown_cmark::TagEnd::Strong => {
                                            style.strong = false;
                                        }
                                        pulldown_cmark::TagEnd::Emphasis => {
                                            style.emphasis = false;
                                        }
                                        pulldown_cmark::TagEnd::Strikethrough => {
                                            style.strikethrough = false;
                                        }
                                        _ => {}
                                    },
                                    pulldown_cmark::Event::Text(s)
                                    | pulldown_cmark::Event::InlineHtml(s)
                                    | pulldown_cmark::Event::Html(s) => {
                                        if in_image {
                                            // Alt text only shown on hover —
                                            // not part of the cell's flow.
                                            continue;
                                        }
                                        let run = Run {
                                            s: s.to_string(),
                                            style: style.clone(),
                                        };
                                        if in_link {
                                            link_runs.push(run);
                                        } else {
                                            widgets.push(Widget::Text(run));
                                        }
                                    }
                                    pulldown_cmark::Event::Code(s) => {
                                        if in_image {
                                            continue;
                                        }
                                        // Renderer toggles `code` around the
                                        // single Code event (see `event`
                                        // dispatch).
                                        let mut run_style = style.clone();
                                        run_style.code = true;
                                        let run = Run {
                                            s: s.to_string(),
                                            style: run_style,
                                        };
                                        if in_link {
                                            link_runs.push(run);
                                        } else {
                                            widgets.push(Widget::Text(run));
                                        }
                                    }
                                    pulldown_cmark::Event::SoftBreak => {
                                        if !in_link && !in_image {
                                            widgets.push(Widget::SoftBreak);
                                        }
                                    }
                                    pulldown_cmark::Event::HardBreak => {
                                        if !in_link && !in_image {
                                            widgets.push(Widget::HardBreak);
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            if widgets.is_empty() {
                                return (0.0, 0.0);
                            }

                            let body_id =
                                Style::default().resolve_font_id(ui_style, strong_family);
                            let space_w = f
                                .layout_no_wrap(" ".to_string(), body_id.clone(), color)
                                .intrinsic_size()
                                .x;

                            let measure_run = |f: &mut egui::epaint::text::FontsView<'_>,
                                                   run: &Run,
                                                   min_w: &mut f32,
                                                   job: Option<&mut egui::text::LayoutJob>|
                             -> f32 {
                                let font = run.style.resolve_font_id(ui_style, strong_family);
                                for word in run.s.split_whitespace() {
                                    if word.is_empty() {
                                        continue;
                                    }
                                    let g = f.layout_no_wrap(
                                        word.to_string(),
                                        font.clone(),
                                        color,
                                    );
                                    *min_w = min_w.max(g.intrinsic_size().x);
                                }
                                if let Some(job) = job {
                                    job.append(
                                        &run.s,
                                        0.0,
                                        egui::TextFormat::simple(font, color),
                                    );
                                    0.0
                                } else {
                                    f.layout_no_wrap(run.s.clone(), font, color)
                                        .intrinsic_size()
                                        .x
                                }
                            };

                            let mut widths: Vec<f32> = Vec::with_capacity(widgets.len());
                            let mut min_w = 0.0_f32;
                            for w in &widgets {
                                match w {
                                    Widget::Text(run) => {
                                        widths.push(measure_run(f, run, &mut min_w, None));
                                    }
                                    Widget::SoftBreak => {
                                        widths.push(space_w);
                                    }
                                    Widget::HardBreak => {
                                        // Row terminator: zero horizontal
                                        // contribution and excluded from the
                                        // inter-widget spacing count below.
                                        widths.push(0.0);
                                    }
                                    Widget::Link(runs) => {
                                        // Render path collapses link runs
                                        // into one LayoutJob and emits one
                                        // widget — measure the same way.
                                        let mut job = egui::text::LayoutJob::default();
                                        job.wrap.max_width = f32::INFINITY;
                                        for run in runs {
                                            measure_run(f, run, &mut min_w, Some(&mut job));
                                        }
                                        widths.push(f.layout_job(job).intrinsic_size().x);
                                    }
                                }
                            }

                            // K_effective excludes hard breaks (they terminate
                            // the row rather than sitting on it).
                            let k_eff = widgets
                                .iter()
                                .filter(|w| !matches!(w, Widget::HardBreak))
                                .count();
                            let gap_total =
                                (k_eff.saturating_sub(1)) as f32 * spacing_x;
                            let natural: f32 = widths.iter().sum::<f32>() + gap_total;
                            if min_w == 0.0 {
                                min_w = natural;
                            }
                            (natural, min_w)
                        }

                        let (col_natural, col_min) = ui.fonts_mut(|f| {
                            let mut col_natural = vec![0.0_f32; num_cols];
                            let mut col_min = vec![0.0_f32; num_cols];
                            for (c, col) in header.iter().take(num_cols).enumerate() {
                                let (n, m) = measure_cell(f, col, &ui_style, strong_family, spacing_x);
                                col_natural[c] = col_natural[c].max(n);
                                col_min[c] = col_min[c].max(m);
                            }
                            for row in &rows {
                                for (c, col) in row.iter().take(num_cols).enumerate() {
                                    let (n, m) = measure_cell(f, col, &ui_style, strong_family, spacing_x);
                                    col_natural[c] = col_natural[c].max(n);
                                    col_min[c] = col_min[c].max(m);
                                }
                            }
                            (col_natural, col_min)
                        });
                        cached.content_hash = content_hash;
                        cached.col_natural = col_natural;
                        cached.col_min = col_min;
                    }
                }

                // Snapshot cached metrics into local arrays so we can release
                // the &mut CommonMarkCache borrow before rendering (cells
                // pass `cache` down to `self.event`).
                let (col_natural, col_min): (Vec<f32>, Vec<f32>) = {
                    let cached = table_cache(cache, &id);
                    let n = cached.col_natural.clone();
                    let m = cached
                        .col_min
                        .iter()
                        .map(|&w| w.max(min_col_floor))
                        .collect::<Vec<_>>();
                    (n, m)
                };

                // Three-case allocation.
                let sum_natural: f32 = col_natural.iter().sum();
                let sum_min: f32 = col_min.iter().sum();
                let col_w: Vec<f32> = if sum_natural <= usable {
                    // A: everything fits unwrapped.
                    col_natural.clone()
                } else if sum_min >= usable {
                    // C: even minimums overflow; word-breaking unavoidable.
                    let scale = usable / sum_min.max(1.0);
                    col_min.iter().map(|&w| w * scale).collect()
                } else {
                    // B: water-fill from min toward natural, weighted by
                    // each column's remaining growth potential. Iterates
                    // because columns may cap at `col_natural` and free up
                    // budget for the rest.
                    let mut widths = col_min.clone();
                    let mut remaining = usable - sum_min;
                    let epsilon = 0.5_f32;
                    for _ in 0..16 {
                        if remaining <= epsilon {
                            break;
                        }
                        let mut growth_potential = 0.0_f32;
                        for c in 0..num_cols {
                            growth_potential += (col_natural[c] - widths[c]).max(0.0);
                        }
                        if growth_potential <= epsilon {
                            break;
                        }
                        let mut grew = 0.0_f32;
                        for c in 0..num_cols {
                            let cap = (col_natural[c] - widths[c]).max(0.0);
                            if cap <= 0.0 {
                                continue;
                            }
                            let share = remaining * (cap / growth_potential);
                            let grow = share.min(cap);
                            widths[c] += grow;
                            grew += grow;
                        }
                        remaining -= grew;
                        if grew <= epsilon {
                            break;
                        }
                    }
                    widths
                };

                // Skip Start/End TableCell events: parse_row leaks an
                // End(TableCell) to the head of every column after the first,
                // and the upstream end_tag handler emits `ui.label("  ")` for
                // it as a Grid-cell separator hack — that hack renders two
                // visible, selectable spaces before the cell content here.
                fn is_cell_delim(e: &pulldown_cmark::Event<'_>) -> bool {
                    matches!(
                        e,
                        pulldown_cmark::Event::Start(pulldown_cmark::Tag::TableCell)
                            | pulldown_cmark::Event::End(pulldown_cmark::TagEnd::TableCell)
                    )
                }

                // Tight content width: sum of column widths plus the gaps
                // between cells. Used to bound the table's vertical layout
                // so the surrounding `Frame::group` and the in-table
                // `separator` don't stretch to the parent's full width.
                let content_width: f32 = col_w.iter().sum::<f32>()
                    + spacing_x * (num_cols.saturating_sub(1)) as f32;

                ui.allocate_ui_with_layout(
                    egui::vec2(content_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |v_ui| {
                    v_ui.set_max_width(content_width);
                    v_ui.spacing_mut().item_spacing.y = 4.0;

                    v_ui.with_layout(
                        egui::Layout::left_to_right(egui::Align::Min),
                        |h_ui| {
                        h_ui.spacing_mut().item_spacing.x = spacing_x;
                        for (c, col) in header.into_iter().take(num_cols).enumerate() {
                            let w = col_w[c];
                            h_ui.allocate_ui_with_layout(
                                egui::vec2(w, 0.0),
                                egui::Layout::left_to_right(egui::Align::Min)
                                    .with_main_wrap(true),
                                |cell_ui| {
                                    cell_ui.set_width(w);
                                    for (e, src_span) in col {
                                        if is_cell_delim(&e) {
                                            continue;
                                        }
                                        let tmp_start = std::mem::replace(
                                            &mut self.line.should_start_newline,
                                            false,
                                        );
                                        let tmp_end = std::mem::replace(
                                            &mut self.line.should_end_newline,
                                            false,
                                        );
                                        self.event(cell_ui, e, src_span, cache, options, w);
                                        self.line.should_start_newline = tmp_start;
                                        self.line.should_end_newline = tmp_end;
                                    }
                                },
                            );
                        }
                    });
                    v_ui.separator();

                    let stripe_color = v_ui.visuals().faint_bg_color;
                    for (row_idx, row) in rows.into_iter().enumerate() {
                        let row_start = v_ui.cursor().min;
                        let row_resp = v_ui.with_layout(
                            egui::Layout::left_to_right(egui::Align::Min),
                            |h_ui| {
                            h_ui.spacing_mut().item_spacing.x = spacing_x;
                            for (c, col) in row.into_iter().take(num_cols).enumerate() {
                                let w = col_w[c];
                                h_ui.allocate_ui_with_layout(
                                    egui::vec2(w, 0.0),
                                    egui::Layout::left_to_right(egui::Align::Min)
                                        .with_main_wrap(true),
                                    |cell_ui| {
                                        cell_ui.set_width(w);
                                        for (e, src_span) in col {
                                            if is_cell_delim(&e) {
                                                continue;
                                            }
                                            let tmp_start = std::mem::replace(
                                                &mut self.line.should_start_newline,
                                                false,
                                            );
                                            let tmp_end = std::mem::replace(
                                                &mut self.line.should_end_newline,
                                                false,
                                            );
                                            self.event(cell_ui, e, src_span, cache, options, w);
                                            self.line.should_start_newline = tmp_start;
                                            self.line.should_end_newline = tmp_end;
                                        }
                                    },
                                );
                            }
                        });
                        if row_idx % 2 == 1 {
                            let row_rect = egui::Rect::from_min_max(
                                egui::pos2(row_start.x, row_start.y),
                                egui::pos2(
                                    row_start.x + row_resp.response.rect.width(),
                                    row_resp.response.rect.bottom(),
                                ),
                            );
                            v_ui.painter().rect_filled(row_rect, 0.0, stripe_color);
                        }
                    }
                });
            });

            self.is_table = false;
            if events.peek().is_none() {
                self.line.should_end_newline_forced = false;
            }

            self.line.try_insert_end(ui);
        }
    }

    fn event(
        &mut self,
        ui: &mut Ui,
        event: pulldown_cmark::Event,
        src_span: Range<usize>,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        match event {
            pulldown_cmark::Event::Start(tag) => self.start_tag(ui, tag, options),
            pulldown_cmark::Event::End(tag) => self.end_tag(ui, tag, cache, options, max_width),
            pulldown_cmark::Event::Text(text) => {
                self.event_text(text, ui, options);
            }
            pulldown_cmark::Event::Code(text) => {
                self.text_style.code = true;
                self.event_text(text, ui, options);
                self.text_style.code = false;
            }
            pulldown_cmark::Event::InlineHtml(text) => {
                self.event_text(text, ui, options);
            }

            pulldown_cmark::Event::Html(text) => {
                if options.html_fn.is_some() {
                    self.html_block.push_str(&text);
                } else {
                    self.event_text(text, ui, options);
                }
            }
            pulldown_cmark::Event::FootnoteReference(footnote) => {
                footnote_start(ui, &footnote);
            }
            pulldown_cmark::Event::SoftBreak => {
                soft_break(ui);
            }
            pulldown_cmark::Event::HardBreak => newline(ui),
            pulldown_cmark::Event::Rule => {
                self.line.try_insert_start(ui);
                rule(ui, self.line.can_insert_end());
            }
            pulldown_cmark::Event::TaskListMarker(mut checkbox) => {
                if options.mutable {
                    if ui
                        .add(egui::Checkbox::without_text(&mut checkbox))
                        .clicked()
                    {
                        self.checkbox_events.push(CheckboxClickEvent {
                            checked: checkbox,
                            span: src_span,
                        });
                    }
                } else {
                    ui.add(ImmutableCheckbox::without_text(&mut checkbox));
                }
            }
            pulldown_cmark::Event::InlineMath(tex) => {
                if let Some(math_fn) = options.math_fn {
                    math_fn(ui, &tex, true);
                }
            }
            pulldown_cmark::Event::DisplayMath(tex) => {
                if let Some(math_fn) = options.math_fn {
                    math_fn(ui, &tex, false);
                }
            }
        }
    }

    fn event_text(&mut self, text: CowStr, ui: &mut Ui, options: &CommonMarkOptions) {
        let rich_text = self.text_style.to_richtext_with(
            ui,
            &text,
            options.strong_font_family.as_ref(),
        );
        if let Some(image) = &mut self.image {
            image.alt_text.push(rich_text);
        } else if let Some(block) = &mut self.code_block {
            block.content.push_str(&text);
        } else if let Some(link) = &mut self.link {
            link.text.push(rich_text);
        } else {
            ui.label(rich_text);
        }
    }

    fn start_tag(&mut self, ui: &mut Ui, tag: pulldown_cmark::Tag, options: &CommonMarkOptions) {
        match tag {
            pulldown_cmark::Tag::Paragraph => {
                self.line.try_insert_start(ui);
            }
            pulldown_cmark::Tag::Heading { level, .. } => {
                // Headings should always insert a newline even if it is at the start.
                // Whether this is okay in all scenarios is a different question.
                newline(ui);
                self.text_style.heading = Some(match level {
                    HeadingLevel::H1 => 0,
                    HeadingLevel::H2 => 1,
                    HeadingLevel::H3 => 2,
                    HeadingLevel::H4 => 3,
                    HeadingLevel::H5 => 4,
                    HeadingLevel::H6 => 5,
                });
            }

            // deliberately not using the built in alerts from pulldown-cmark as
            // the markdown itself cannot be localized :( e.g: [!TIP]
            pulldown_cmark::Tag::BlockQuote(_) => {
                self.is_blockquote = true;
            }
            pulldown_cmark::Tag::CodeBlock(c) => {
                match c {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                        self.code_block = Some(crate::CodeBlock {
                            lang: Some(lang.to_string()),
                            content: "".to_string(),
                        });
                    }
                    pulldown_cmark::CodeBlockKind::Indented => {
                        self.code_block = Some(crate::CodeBlock {
                            lang: None,
                            content: "".to_string(),
                        });
                    }
                }
                self.line.try_insert_start(ui);
            }

            pulldown_cmark::Tag::List(point) => {
                if !self.list.is_inside_a_list() && self.line.can_insert_start() {
                    newline(ui);
                }

                if let Some(number) = point {
                    self.list.start_level_with_number(number);
                } else {
                    self.list.start_level_without_number();
                }
                self.line.should_start_newline = false;
                self.line.should_end_newline = false;
            }

            pulldown_cmark::Tag::Item => {
                self.is_list_item = true;
                self.list.start_item(ui, options);
            }

            pulldown_cmark::Tag::FootnoteDefinition(note) => {
                self.line.try_insert_start(ui);

                self.line.should_start_newline = false;
                self.line.should_end_newline = false;
                footnote(ui, &note);
            }
            pulldown_cmark::Tag::Table(_) => {
                self.is_table = true;
            }
            pulldown_cmark::Tag::TableHead => {}
            pulldown_cmark::Tag::TableRow => {}
            pulldown_cmark::Tag::TableCell => {}
            pulldown_cmark::Tag::Emphasis => {
                self.text_style.emphasis = true;
            }
            pulldown_cmark::Tag::Strong => {
                self.text_style.strong = true;
            }
            pulldown_cmark::Tag::Strikethrough => {
                self.text_style.strikethrough = true;
            }
            pulldown_cmark::Tag::Link { dest_url, .. } => {
                self.link = Some(crate::Link {
                    destination: dest_url.to_string(),
                    text: Vec::new(),
                });
            }
            pulldown_cmark::Tag::Image { dest_url, .. } => {
                self.image = Some(crate::Image::new(&dest_url, options));
            }
            pulldown_cmark::Tag::HtmlBlock => {
                self.line.try_insert_start(ui);
            }
            pulldown_cmark::Tag::MetadataBlock(_) => {}

            pulldown_cmark::Tag::DefinitionList => {
                self.line.try_insert_start(ui);
                self.def_list.is_first_item = true;
            }
            pulldown_cmark::Tag::DefinitionListTitle => {
                // we disable newline as the first title should not insert a newline
                // as we have already done that upon the DefinitionList Tag
                if !self.def_list.is_first_item {
                    self.line.try_insert_start(ui)
                } else {
                    self.def_list.is_first_item = false;
                }
            }
            pulldown_cmark::Tag::DefinitionListDefinition => {
                self.def_list.is_def_list_def = true;
            }
            // Not yet supported
            pulldown_cmark::Tag::Superscript | pulldown_cmark::Tag::Subscript => {}
        }
    }

    fn end_tag(
        &mut self,
        ui: &mut Ui,
        tag: pulldown_cmark::TagEnd,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        match tag {
            pulldown_cmark::TagEnd::Paragraph => {
                self.line.try_insert_end(ui);
            }
            pulldown_cmark::TagEnd::Heading { .. } => {
                self.line.try_insert_end(ui);
                self.text_style.heading = None;
            }
            pulldown_cmark::TagEnd::BlockQuote(_) => {}
            pulldown_cmark::TagEnd::CodeBlock => {
                self.end_code_block(ui, cache, options, max_width);
            }

            pulldown_cmark::TagEnd::List(_) => {
                if self.list.is_last_level() {
                    self.line.should_start_newline = true;
                    self.line.should_end_newline = true;
                }

                self.list.end_level(ui, self.line.can_insert_end());

                if !self.list.is_inside_a_list() {
                    // Reset all the state and make it ready for the next list that occurs
                    self.list = List::default();
                }
            }
            pulldown_cmark::TagEnd::Item => {}
            pulldown_cmark::TagEnd::FootnoteDefinition => {
                self.line.should_start_newline = true;
                self.line.should_end_newline = true;
                self.line.try_insert_end(ui);
            }
            pulldown_cmark::TagEnd::Table => {}
            pulldown_cmark::TagEnd::TableHead => {}
            pulldown_cmark::TagEnd::TableRow => {}
            pulldown_cmark::TagEnd::TableCell => {
                // Ensure space between cells
                ui.label("  ");
            }
            pulldown_cmark::TagEnd::Emphasis => {
                self.text_style.emphasis = false;
            }
            pulldown_cmark::TagEnd::Strong => {
                self.text_style.strong = false;
            }
            pulldown_cmark::TagEnd::Strikethrough => {
                self.text_style.strikethrough = false;
            }
            pulldown_cmark::TagEnd::Link => {
                if let Some(link) = self.link.take() {
                    link.end(ui, cache);
                }
            }
            pulldown_cmark::TagEnd::Image => {
                if let Some(image) = self.image.take() {
                    image.end(ui, options);
                }
            }
            pulldown_cmark::TagEnd::HtmlBlock => {
                if let Some(html_fn) = options.html_fn {
                    html_fn(ui, &self.html_block);
                    self.html_block.clear();
                }
            }

            pulldown_cmark::TagEnd::MetadataBlock(_) => {}

            pulldown_cmark::TagEnd::DefinitionList => self.line.try_insert_end(ui),
            pulldown_cmark::TagEnd::DefinitionListTitle
            | pulldown_cmark::TagEnd::DefinitionListDefinition => {}
            pulldown_cmark::TagEnd::Superscript | pulldown_cmark::TagEnd::Subscript => {}
        }
    }

    fn end_code_block(
        &mut self,
        ui: &mut Ui,
        cache: &mut CommonMarkCache,
        options: &CommonMarkOptions,
        max_width: f32,
    ) {
        if let Some(block) = self.code_block.take() {
            block.end(ui, cache, options, max_width);
            self.line.try_insert_end(ui);
        }
    }
}
