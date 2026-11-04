//! DirectWrite text: fonts, cached layouts, measurement, and hit testing.
//!
//! Layouts are cached by their text and options, so a grid that redraws the
//! same cells each frame reuses them. Offsets in this module's API are UTF-8
//! byte offsets into the laid-out text; DirectWrite's UTF-16 positions stay
//! inside.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{HSTRING, w};

use super::geometry::{Rect, rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Weight {
    #[default]
    Normal,
    Semibold,
    Bold,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub family: Arc<str>,
    pub size: f32,
    pub weight: Weight,
    pub italic: bool,
}

impl Font {
    pub fn new(family: impl Into<Arc<str>>, size: f32) -> Self {
        Self {
            family: family.into(),
            size,
            weight: Weight::Normal,
            italic: false,
        }
    }

    pub fn weight(&self, weight: Weight) -> Self {
        Self {
            weight,
            ..self.clone()
        }
    }

    pub fn bold(&self) -> Self {
        self.weight(Weight::Bold)
    }

    pub fn semibold(&self) -> Self {
        self.weight(Weight::Semibold)
    }

    pub fn sized(&self, size: f32) -> Self {
        Self {
            size,
            ..self.clone()
        }
    }

    pub fn family(&self, family: impl Into<Arc<str>>) -> Self {
        Self {
            family: family.into(),
            ..self.clone()
        }
    }

    fn key(&self) -> FormatKey {
        FormatKey {
            family: self.family.clone(),
            size: self.size.to_bits(),
            weight: self.weight,
            italic: self.italic,
        }
    }
}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct FormatKey {
    family: Arc<str>,
    size: u32,
    weight: Weight,
    italic: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// How a layout fits its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextOptions {
    /// `f32::INFINITY` lays the text out on lines as long as it needs.
    pub max_width: f32,
    pub wrap: bool,
    /// Ends text cut off at `max_width` with `…`. Needs `wrap` off.
    pub ellipsis: bool,
    pub align: Align,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            max_width: f32::INFINITY,
            wrap: false,
            ellipsis: false,
            align: Align::Left,
        }
    }
}

impl TextOptions {
    pub fn wrapped(width: f32) -> Self {
        Self {
            max_width: width.max(1.0),
            wrap: true,
            ..Default::default()
        }
    }

    pub fn truncated(width: f32) -> Self {
        Self {
            max_width: width.max(1.0),
            ellipsis: true,
            ..Default::default()
        }
    }

    pub fn align(self, align: Align) -> Self {
        Self { align, ..self }
    }
}

/// One laid-out piece of text.
pub struct TextLayout {
    pub(crate) layout: IDWriteTextLayout,
    text: String,
    /// The UTF-16 index of each UTF-8 byte offset that starts a character,
    /// plus the end.
    utf16: Vec<(usize, u32)>,
    pub width: f32,
    pub height: f32,
    pub line_count: usize,
    /// The layout's box width: `max_width`, or the text width when unbounded.
    pub layout_width: f32,
}

impl TextLayout {
    pub fn text(&self) -> &str {
        &self.text
    }

    fn to_utf16(&self, offset: usize) -> u32 {
        match self.utf16.binary_search_by_key(&offset, |(byte, _)| *byte) {
            Ok(index) => self.utf16[index].1,
            Err(index) => self.utf16[index.saturating_sub(1)].1,
        }
    }

    fn to_utf8(&self, position: u32) -> usize {
        match self.utf16.binary_search_by_key(&position, |(_, unit)| *unit) {
            Ok(index) => self.utf16[index].0,
            Err(index) => self.utf16[index.saturating_sub(1)].0,
        }
    }

    /// The byte offset nearest `(x, y)`, relative to the layout's origin.
    pub fn hit_test(&self, x: f32, y: f32) -> usize {
        let mut trailing = windows::core::BOOL::default();
        let mut inside = windows::core::BOOL::default();
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            if self
                .layout
                .HitTestPoint(x, y, &mut trailing, &mut inside, &mut metrics)
                .is_err()
            {
                return 0;
            }
        }
        let position = metrics.textPosition + if trailing.as_bool() { metrics.length } else { 0 };
        self.to_utf8(position).min(self.text.len())
    }

    /// The caret position before byte `offset`: x, the line's top, and its height.
    pub fn caret(&self, offset: usize) -> (f32, f32, f32) {
        let position = self.to_utf16(offset.min(self.text.len()));
        let (mut x, mut y) = (0.0, 0.0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self
                .layout
                .HitTestTextPosition(position, false, &mut x, &mut y, &mut metrics);
        }
        (x, metrics.top, metrics.height)
    }

    /// Rectangles covering bytes `start..end`, relative to the layout's origin.
    pub fn range_rects(&self, start: usize, end: usize) -> Vec<Rect> {
        if end <= start {
            return Vec::new();
        }
        let start = self.to_utf16(start);
        let end = self.to_utf16(end.min(self.text.len()));
        if end <= start {
            return Vec::new();
        }
        let mut count = 0u32;
        unsafe {
            let _ = self
                .layout
                .HitTestTextRange(start, end - start, 0.0, 0.0, None, &mut count);
        }
        if count == 0 {
            return Vec::new();
        }
        let mut metrics = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
        unsafe {
            if self
                .layout
                .HitTestTextRange(
                    start,
                    end - start,
                    0.0,
                    0.0,
                    Some(&mut metrics),
                    &mut count,
                )
                .is_err()
            {
                return Vec::new();
            }
        }
        metrics
            .iter()
            .take(count as usize)
            .map(|m| rect(m.left, m.top, m.width, m.height))
            .collect()
    }

    /// Whether the text was cut short to fit its width.
    pub fn is_trimmed(&self) -> bool {
        self.width > self.layout_width + 0.5
    }

    pub(crate) fn range(&self, start: usize, end: usize) -> DWRITE_TEXT_RANGE {
        let start = self.to_utf16(start);
        let end = self.to_utf16(end.min(self.text.len()));
        DWRITE_TEXT_RANGE {
            startPosition: start,
            length: end.saturating_sub(start),
        }
    }
}

struct CacheEntry {
    layout: Rc<TextLayout>,
    font: Font,
    options: TextOptions,
    used: u64,
}

pub struct TextSystem {
    factory: IDWriteFactory,
    formats: HashMap<FormatKey, IDWriteTextFormat>,
    cache: HashMap<u64, Vec<CacheEntry>>,
    cache_len: usize,
    frame: u64,
    families: Option<Vec<String>>,
    /// The system fonts plus those installed for this user only, which the
    /// system collection leaves out. Built the first time it is needed.
    all_fonts: Option<Option<IDWriteFontCollection>>,
}

thread_local! {
    // Never dropped: releasing DirectWrite objects while the process exits
    // can fail inside the system DLLs.
    static TEXT: std::mem::ManuallyDrop<RefCell<Option<TextSystem>>> =
        const { std::mem::ManuallyDrop::new(RefCell::new(None)) };
}

/// Runs `f` with this thread's text system.
pub fn with<R>(f: impl FnOnce(&mut TextSystem) -> R) -> R {
    TEXT.with(|text| {
        let mut text = text.borrow_mut();
        let system = text.get_or_insert_with(|| TextSystem::new().expect("create DirectWrite factory"));
        f(system)
    })
}

/// Lays out `text`, reusing an identical earlier layout.
pub fn layout(text: &str, font: &Font, options: TextOptions) -> Rc<TextLayout> {
    with(|system| system.layout(text, font, options))
}

/// How part of a styled layout differs from the layout's font.
#[derive(Clone, Debug, PartialEq)]
pub enum RunStyle {
    Bold,
    Semibold,
    Italic,
    Underline,
    /// Another family and size, such as code inside a paragraph.
    Font(Font),
}

/// Lays out `text` with byte ranges styled by `runs`. Not cached: callers
/// keep the layout while the text and width stay the same.
pub fn styled_layout(text: &str, font: &Font, options: TextOptions, runs: &[(std::ops::Range<usize>, RunStyle)]) -> Rc<TextLayout> {
    with(|system| Rc::new(system.styled_layout(text, font, options, runs)))
}

/// The width and height of `text` on one line.
pub fn measure(text: &str, font: &Font) -> (f32, f32) {
    let layout = layout(text, font, TextOptions::default());
    (layout.width, layout.height)
}

/// The height of one line of `font`.
pub fn line_height(font: &Font) -> f32 {
    measure("Ag", font).1
}

pub fn font_families() -> Vec<String> {
    with(|system| system.font_families())
}

fn hash_key(text: &str, font: &Font, options: &TextOptions) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    font.hash(&mut hasher);
    options.max_width.to_bits().hash(&mut hasher);
    options.wrap.hash(&mut hasher);
    options.ellipsis.hash(&mut hasher);
    options.align.hash(&mut hasher);
    hasher.finish()
}

impl TextSystem {
    fn new() -> windows::core::Result<Self> {
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        Ok(Self {
            factory,
            formats: HashMap::new(),
            cache: HashMap::new(),
            cache_len: 0,
            frame: 0,
            families: None,
            all_fonts: None,
        })
    }

    pub fn factory(&self) -> &IDWriteFactory {
        &self.factory
    }

    fn format(&mut self, font: &Font) -> IDWriteTextFormat {
        let key = font.key();
        if let Some(format) = self.formats.get(&key) {
            return format.clone();
        }
        let weight = match font.weight {
            Weight::Normal => DWRITE_FONT_WEIGHT_NORMAL,
            Weight::Semibold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
            Weight::Bold => DWRITE_FONT_WEIGHT_BOLD,
        };
        let style = if font.italic {
            DWRITE_FONT_STYLE_ITALIC
        } else {
            DWRITE_FONT_STYLE_NORMAL
        };
        let family = HSTRING::from(&*font.family);
        let collection = self.collection_for(&family);
        let format = unsafe {
            self.factory
                .CreateTextFormat(
                    &family,
                    collection.as_ref(),
                    weight,
                    style,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font.size.max(1.0),
                    w!("en-us"),
                )
                .or_else(|_| {
                    self.factory.CreateTextFormat(
                        w!("Segoe UI"),
                        None,
                        weight,
                        style,
                        DWRITE_FONT_STRETCH_NORMAL,
                        font.size.max(1.0),
                        w!("en-us"),
                    )
                })
                .expect("create a text format")
        };
        self.formats.insert(key, format.clone());
        format
    }

    pub fn layout(&mut self, text: &str, font: &Font, options: TextOptions) -> Rc<TextLayout> {
        let key = hash_key(text, font, &options);
        let frame = self.frame;
        if let Some(entries) = self.cache.get_mut(&key)
            && let Some(entry) = entries.iter_mut().find(|entry| {
                entry.layout.text == text && entry.font == *font && entry.options == options
            })
        {
            entry.used = frame;
            return entry.layout.clone();
        }
        let layout = Rc::new(self.create_layout(text, font, options));
        self.cache.entry(key).or_default().push(CacheEntry {
            layout: layout.clone(),
            font: font.clone(),
            options,
            used: frame,
        });
        self.cache_len += 1;
        layout
    }

    fn create_layout(&mut self, text: &str, font: &Font, options: TextOptions) -> TextLayout {
        let format = self.format(font);
        let mut wide = Vec::with_capacity(text.len());
        let mut utf16 = Vec::with_capacity(text.len() + 1);
        for (offset, ch) in text.char_indices() {
            utf16.push((offset, wide.len() as u32));
            let mut buffer = [0u16; 2];
            wide.extend_from_slice(ch.encode_utf16(&mut buffer));
        }
        utf16.push((text.len(), wide.len() as u32));
        let unbounded = !options.max_width.is_finite();
        let max_width = if unbounded { 1.0e7 } else { options.max_width.max(1.0) };
        let layout = unsafe {
            self.factory
                .CreateTextLayout(&wide, &format, max_width, 1.0e7)
                .expect("create a text layout")
        };
        unsafe {
            let _ = layout.SetWordWrapping(if options.wrap {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            });
            if options.ellipsis && !unbounded {
                if let Ok(sign) = self.factory.CreateEllipsisTrimmingSign(&format) {
                    let trimming = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = layout.SetTrimming(&trimming, &sign);
                }
            }
            if !unbounded {
                let _ = layout.SetTextAlignment(match options.align {
                    Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                    Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                    Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                });
            }
            let _ = layout.SetIncrementalTabStop(font.size * 2.0);
        }
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut metrics);
        }
        // A trimmed layout reports its trimmed width, so measure the whole text too.
        let mut width = metrics.widthIncludingTrailingWhitespace;
        if options.ellipsis && !unbounded {
            let full = unsafe { self.factory.CreateTextLayout(&wide, &format, 1.0e7, 1.0e7) };
            if let Ok(full) = full {
                unsafe {
                    let _ = full.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                    let mut full_metrics = DWRITE_TEXT_METRICS::default();
                    let _ = full.GetMetrics(&mut full_metrics);
                    width = full_metrics.widthIncludingTrailingWhitespace;
                }
            }
        }
        if unbounded {
            unsafe {
                let _ = layout.SetMaxWidth(width.max(1.0));
            }
        }
        TextLayout {
            layout,
            text: text.to_owned(),
            utf16,
            width,
            height: metrics.height,
            line_count: metrics.lineCount as usize,
            layout_width: if unbounded { width } else { max_width },
        }
    }

    fn styled_layout(&mut self, text: &str, font: &Font, options: TextOptions, runs: &[(std::ops::Range<usize>, RunStyle)]) -> TextLayout {
        let mut layout = self.create_layout(text, font, options);
        if runs.is_empty() {
            return layout;
        }
        for (range, style) in runs {
            let range = layout.range(range.start, range.end);
            unsafe {
                let _ = match style {
                    RunStyle::Bold => layout.layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range),
                    RunStyle::Semibold => layout.layout.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, range),
                    RunStyle::Italic => layout.layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range),
                    RunStyle::Underline => layout.layout.SetUnderline(true, range),
                    RunStyle::Font(other) => {
                        let family = HSTRING::from(&*other.family);
                        if let Some(collection) = self.collection_for(&family) {
                            let _ = layout.layout.SetFontCollection(&collection, range);
                        }
                        let _ = layout.layout.SetFontSize(other.size.max(1.0), range);
                        layout.layout.SetFontFamilyName(&family, range)
                    }
                };
            }
        }
        // The styles change the text's size.
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.layout.GetMetrics(&mut metrics);
        }
        layout.width = metrics.widthIncludingTrailingWhitespace;
        layout.height = metrics.height;
        layout.line_count = metrics.lineCount as usize;
        if !options.max_width.is_finite() {
            unsafe {
                let _ = layout.layout.SetMaxWidth(layout.width.max(1.0));
            }
            layout.layout_width = layout.width;
        }
        layout
    }

    /// Marks the start of a frame; layouts unused for a few frames are
    /// dropped once the cache grows large.
    pub fn next_frame(&mut self) {
        self.frame += 1;
        if self.cache_len > 20_000 {
            let keep_after = self.frame.saturating_sub(3);
            let mut len = 0;
            self.cache.retain(|_, entries| {
                entries.retain(|entry| entry.used >= keep_after);
                len += entries.len();
                !entries.is_empty()
            });
            self.cache_len = len;
        }
    }

    fn system_fonts(&self) -> Option<IDWriteFontCollection> {
        let mut collection = None;
        unsafe { self.factory.GetSystemFontCollection(&mut collection, false).ok()? };
        collection
    }

    fn all_fonts(&mut self) -> Option<IDWriteFontCollection> {
        if self.all_fonts.is_none() {
            self.all_fonts = Some(merged_fonts(&self.factory));
        }
        self.all_fonts.clone().flatten()
    }

    /// The collection to find `family` in: the system one (`None`) unless only
    /// the per-user fonts have it, so those are not loaded without need.
    fn collection_for(&mut self, family: &HSTRING) -> Option<IDWriteFontCollection> {
        if self.system_fonts().is_some_and(|fonts| has_family(&fonts, family)) {
            None
        } else {
            self.all_fonts()
        }
    }

    pub fn font_families(&mut self) -> Vec<String> {
        if let Some(families) = &self.families {
            return families.clone();
        }
        let mut names = Vec::new();
        unsafe {
            if let Some(collection) = self.all_fonts().or_else(|| self.system_fonts()) {
                for index in 0..collection.GetFontFamilyCount() {
                    let Ok(family) = collection.GetFontFamily(index) else {
                        continue;
                    };
                    let Ok(localized) = family.GetFamilyNames() else {
                        continue;
                    };
                    let mut name_index = 0u32;
                    let mut exists = windows::core::BOOL::default();
                    let _ = localized.FindLocaleName(w!("en-us"), &mut name_index, &mut exists);
                    if !exists.as_bool() {
                        name_index = 0;
                    }
                    let Ok(length) = localized.GetStringLength(name_index) else {
                        continue;
                    };
                    let mut buffer = vec![0u16; length as usize + 1];
                    if localized.GetString(name_index, &mut buffer).is_ok() {
                        names.push(String::from_utf16_lossy(&buffer[..length as usize]));
                    }
                }
            }
        }
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        self.families = Some(names.clone());
        names
    }
}

fn has_family(collection: &IDWriteFontCollection, family: &HSTRING) -> bool {
    let mut index = 0;
    let mut exists = windows::core::BOOL::default();
    unsafe { collection.FindFamilyName(family, &mut index, &mut exists) }.is_ok() && exists.as_bool()
}

/// The system font set plus the fonts installed for this user only, or
/// `None` when there are no per-user fonts.
fn merged_fonts(factory: &IDWriteFactory) -> Option<IDWriteFontCollection> {
    use windows::core::Interface;
    let files = super::system::user_font_files();
    if files.is_empty() {
        return None;
    }
    unsafe {
        let factory: IDWriteFactory6 = factory.cast().ok()?;
        let builder = factory.CreateFontSetBuilder().ok()?;
        builder.AddFontSet(&factory.GetSystemFontSet(false).ok()?).ok()?;
        for path in files {
            let _ = builder.AddFontFile(&HSTRING::from(path.as_os_str()));
        }
        let set = builder.CreateFontSet().ok()?;
        IDWriteFactory3::CreateFontCollectionFromFontSet(&factory, &set)
            .ok()?
            .cast()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_user_fonts_join_the_system_fonts() {
        if super::super::system::user_font_files().is_empty() {
            return;
        }
        let mut system = TextSystem::new().unwrap();
        let installed = unsafe { system.system_fonts().unwrap().GetFontFamilyCount() };
        let merged = system.all_fonts().expect("per-user fonts are merged");
        assert!(unsafe { merged.GetFontFamilyCount() } > installed);
    }
}
