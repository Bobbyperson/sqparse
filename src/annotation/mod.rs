//! A utility for pretty-printing source code annotations, warnings and errors.
//!
//! [`display_annotations`] returns an object that implements [`Display`], that will pretty-print
//! some source code based on a list of [`Annotation`]s.
//!
//! # Example
//! ```
//! use sqparse::annotation::{Annotation, display_annotations, Mode};
//!
//! yansi::disable();
//!
//! let source = "highlight me!";
//! let annotations = [
//!     Annotation {
//!         mode: Mode::Info,
//!         text: "this is me!".to_string(),
//!         note: "".to_string(),
//!         highlight: 10..12,
//!         visible: 10..12,
//!     }
//! ];
//! let annotations = format!("{}", display_annotations(Some("file.txt"), source, &annotations));
//! assert_eq!(annotations, " --> file.txt:1:11
//!   |
//! 1 | highlight me!
//!   |           -- this is me!");
//! ```

mod formats;
mod gutter;
mod line_printer;
mod mode;
mod repeat;

use crate::annotation::formats::{MultiLineFormatDisplay, SingleLineFormatDisplay};
use crate::annotation::gutter::Gutter;
use std::fmt::{Display, Formatter};
use std::ops::{Range, RangeInclusive};
use yansi::Paint;

pub use self::mode::Mode;

/// A source code annotation.
///
/// An annotation describes which part of source code to highlight, and how to display it.
#[derive(Debug, Clone)]
pub struct Annotation {
    /// Controls the theme of the annotation, changing colors and some characters.
    pub mode: Mode,

    /// Text to print beside the highlighted region.
    ///
    /// # Example
    /// ```text
    /// 1 | this is the source text
    ///   |      ^^^^^^ this is the annotation text
    /// ```
    pub text: String,

    /// Text to print as a note underneath the annotation.
    ///
    /// # Example
    /// ```text
    /// 1 | this is the source text
    ///   |
    ///   = note: this is the note text
    /// ```
    pub note: String,

    /// Byte range of the source text to highlight.
    ///
    /// Offsets are clamped to the source. If an offset falls inside a UTF-8 code point, the start
    /// is rounded down and the end is rounded up to the nearest character boundary.
    ///
    /// If all characters on this range are on the same line, a single line will be printed like
    /// this:
    /// ```text
    /// 1 | this is some source text
    ///          ^^^^^^^^^^^^^^
    /// ```
    ///
    /// If the range spans multiple lines, the first and last two lines will be printed like this:
    /// ```text
    ///  1 |   println(
    ///    |  ________^
    ///  2 | |    "hello",
    /// ..   |
    /// 10 | |   1 + 2,
    /// 11 | | )
    ///    | |_^
    /// ```
    pub highlight: Range<usize>,

    /// Byte range that must be visible in a multi-line output.
    ///
    /// When `highlight` spans multiple lines, lines between the first and last two may be folded.
    /// However any lines covered by the `visible` range will be included. Portions outside
    /// `highlight` are clamped to the highlighted lines.
    ///
    /// For example, this can cause one or more lines to be unfolded:
    /// ```text
    ///  1 |   println(
    ///    |  ________^
    ///  2 | |     "hello",
    /// ..   |
    ///  5 | |     myFunc(),
    /// ..   |
    /// 10 | |     1 + 2,
    /// 11 | | )
    ///    | |_^
    /// ```
    pub visible: Range<usize>,
}

/// Displays a list of annotations from a source string.
///
/// The annotations will be prepended with the file name, if one is provided, and the start line
/// and one-based byte offset of the first annotation. For example:
/// ```text
///   --> my_file.txt:5:1
///  5 | error
///    | ^^^^^
/// ```
///
/// For details on how each annotation is formatted, see [`Annotation`].
///
/// # Example
/// ```
/// use sqparse::annotation::{Annotation, display_annotations, Mode};
///
/// yansi::disable();
///
/// let source = "highlight me!";
/// let annotations = [
///     Annotation {
///         mode: Mode::Info,
///         text: "this is me!".to_string(),
///         note: "".to_string(),
///         highlight: 10..12,
///         visible: 10..12,
///     }
/// ];
/// let annotations = format!("{}", display_annotations(Some("file.txt"), source, &annotations));
/// assert_eq!(annotations, " --> file.txt:1:11
///   |
/// 1 | highlight me!
///   |           -- this is me!");
/// ```
pub fn display_annotations<'s>(
    file_name: Option<&'s str>,
    source: &'s str,
    annotations: &'s [Annotation],
) -> impl Display + 's {
    let format_data: Vec<_> = annotations
        .iter()
        .map(|annotation| {
            FormatData::new(
                source,
                annotation.highlight.clone(),
                annotation.visible.clone(),
            )
        })
        .collect();
    let max_line_number = format_data
        .iter()
        .map(|format| *format.line_numbers().end())
        .max()
        .unwrap_or(0);
    let gutter = Gutter::new(max_line_number);

    AnnotationsDisplay {
        file_name,
        annotations,
        gutter,
        format_data,
    }
}

struct AnnotationsDisplay<'s> {
    file_name: Option<&'s str>,
    annotations: &'s [Annotation],
    gutter: Gutter,
    format_data: Vec<FormatData<'s>>,
}

impl Display for AnnotationsDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ", self.gutter.file())?;
        if let Some(file_name) = self.file_name {
            write!(f, "{file_name}:")?;
        }
        if let Some(first_format_data) = self.format_data.first() {
            write!(
                f,
                "{}:{}",
                first_format_data.line_numbers().start(),
                first_format_data.first_line_highlight() + 1
            )?;
        }

        for (annotation_index, (annotation, format_data)) in self
            .annotations
            .iter()
            .zip(self.format_data.iter())
            .enumerate()
        {
            writeln!(f, "\n{}", self.gutter.empty())?;

            match format_data {
                FormatData::SingleLine {
                    line,
                    line_number,
                    line_highlight,
                } => {
                    write!(
                        f,
                        "{}",
                        SingleLineFormatDisplay {
                            mode: annotation.mode,
                            gutter: self.gutter,
                            line,
                            line_number: *line_number,
                            line_highlight: line_highlight.clone()
                        },
                    )?;
                }
                FormatData::MultiLine {
                    lines,
                    line_numbers,
                    must_be_visible_line_numbers,
                    first_line_highlight,
                    last_line_highlight,
                } => {
                    write!(
                        f,
                        "{}",
                        MultiLineFormatDisplay {
                            mode: annotation.mode,
                            gutter: self.gutter,
                            lines,
                            line_numbers: line_numbers.clone(),
                            must_be_visible_line_numbers: must_be_visible_line_numbers.clone(),
                            first_line_highlight: *first_line_highlight,
                            last_line_highlight: *last_line_highlight,
                        },
                    )?;
                }
            }

            write!(f, " {}", annotation.mode.display(&annotation.text))?;

            if !annotation.note.is_empty() {
                writeln!(f, "\n{}", self.gutter.empty())?;
                write!(
                    f,
                    "{} {}",
                    self.gutter.separator(),
                    Paint::white(&annotation.note).bold()
                )?;
            } else if annotation_index + 1 < self.annotations.len() {
                write!(f, "\n{}", self.gutter.separator())?;
            }
        }

        Ok(())
    }
}

enum FormatData<'s> {
    SingleLine {
        line: &'s str,
        line_number: usize,
        line_highlight: Range<usize>,
    },
    MultiLine {
        lines: &'s str,
        line_numbers: RangeInclusive<usize>,
        must_be_visible_line_numbers: RangeInclusive<usize>,
        first_line_highlight: usize,
        last_line_highlight: usize,
    },
}

impl<'s> FormatData<'s> {
    pub fn new(text: &'s str, highlight: Range<usize>, visible: Range<usize>) -> Self {
        let highlight = normalize_byte_range(text, highlight);
        let visible = normalize_byte_range(text, visible);

        let has_newline = text[highlight.clone()].contains('\n');

        if has_newline {
            Self::new_multi_line(text, highlight, visible)
        } else {
            Self::new_single_line(text, highlight)
        }
    }

    pub fn line_numbers(&self) -> RangeInclusive<usize> {
        match self {
            FormatData::SingleLine { line_number, .. } => (*line_number)..=(*line_number),
            FormatData::MultiLine { line_numbers, .. } => line_numbers.clone(),
        }
    }

    pub fn first_line_highlight(&self) -> usize {
        match self {
            FormatData::SingleLine { line_highlight, .. } => line_highlight.start,
            FormatData::MultiLine {
                first_line_highlight,
                ..
            } => *first_line_highlight,
        }
    }

    fn new_single_line(text: &'s str, highlight: Range<usize>) -> Self {
        let (line_number, line_start_index) = get_line_containing(highlight.start, text);
        let text_from_start = &text[line_start_index..];

        let end_index = text_from_start.find('\n').unwrap_or(text_from_start.len());
        let line = &text_from_start[..end_index];

        let line_highlight =
            (highlight.start - line_start_index)..(highlight.end - line_start_index);

        FormatData::SingleLine {
            line,
            line_number,
            line_highlight,
        }
    }

    fn new_multi_line(text: &'s str, highlight: Range<usize>, visible: Range<usize>) -> Self {
        let (first_line_number, first_start_index) = get_line_containing(highlight.start, text);
        let highlight_last_char = last_char_start(text, highlight.end);
        let (last_line_number, last_start_index) = get_line_containing(highlight_last_char, text);

        // `visible` is only meaningful inside the highlighted region. Clamp its line numbers so
        // ranges entirely before or after the highlight cannot make the formatter underflow.
        let first_must_be_visible_line_number = get_line_containing(visible.start, text)
            .0
            .clamp(first_line_number, last_line_number);
        let last_must_be_visible_line_number =
            get_line_containing(last_char_start(text, visible.end), text)
                .0
                .clamp(first_must_be_visible_line_number, last_line_number);

        let last_end_index = text[last_start_index..]
            .find('\n')
            .map(|idx| last_start_index + idx)
            .unwrap_or(text.len());

        let lines = &text[first_start_index..last_end_index];
        let first_line_highlight = highlight.start - first_start_index;
        let last_line_highlight = highlight_last_char - last_start_index;

        FormatData::MultiLine {
            line_numbers: first_line_number..=last_line_number,
            must_be_visible_line_numbers: first_must_be_visible_line_number
                ..=last_must_be_visible_line_number,
            lines,
            first_line_highlight,
            last_line_highlight,
        }
    }
}

fn normalize_byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = range.start.min(text.len());
    let end = range.end.min(text.len()).max(start);

    let normalized_start = char_boundary_at_or_before(text, start);

    let mut normalized_end = end;
    while !text.is_char_boundary(normalized_end) {
        normalized_end += 1;
    }

    normalized_start..normalized_end
}

fn last_char_start(text: &str, exclusive_end: usize) -> usize {
    let exclusive_end = char_boundary_at_or_before(text, exclusive_end.min(text.len()));
    text[..exclusive_end]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn char_boundary_at_or_before(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn get_line_containing(index: usize, val: &str) -> (usize, usize) {
    let index = char_boundary_at_or_before(val, index);
    let line = val[..index].chars().filter(|ch| *ch == '\n').count() + 1;
    let line_start_index = val[..index].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    (line, line_start_index)
}

#[cfg(test)]
mod tests {
    use super::{Annotation, Mode, display_annotations};

    fn annotation(
        highlight: std::ops::Range<usize>,
        visible: std::ops::Range<usize>,
    ) -> Annotation {
        Annotation {
            mode: Mode::Info,
            text: "note".to_string(),
            note: String::new(),
            highlight,
            visible,
        }
    }

    #[test]
    fn annotations_use_byte_offsets_for_non_ascii_source() {
        yansi::disable();
        let output = format!(
            "{}",
            display_annotations(Some("test.nut"), "éx", &[annotation(2..3, 2..3)])
        );

        assert!(output.starts_with(" --> test.nut:1:3"));
        assert!(output.contains("1 | éx"));
    }

    #[test]
    fn annotation_offsets_inside_utf8_code_points_are_normalized() {
        yansi::disable();
        let output = format!(
            "{}",
            display_annotations(None, "éx", &[annotation(1..2, 1..2)])
        );

        assert!(output.contains("1 | éx"));
        assert!(output.contains("| -- note"));
    }

    #[test]
    fn multiline_visible_range_before_highlight_is_clamped() {
        yansi::disable();
        let source = "outside\noutside\noutside\nfirst\nsecond\nthird\nfourth\noutside\n";
        let highlight_start = source.find("first").unwrap();
        let highlight_end = source.rfind("\noutside\n").unwrap();
        let output = format!(
            "{}",
            display_annotations(
                None,
                source,
                &[annotation(highlight_start..highlight_end, 0..7)]
            )
        );

        assert!(output.contains("first"));
        assert!(output.contains("fourth"));
    }

    #[test]
    fn multiline_visible_range_after_highlight_is_clamped() {
        yansi::disable();
        let output = format!(
            "{}",
            display_annotations(None, "a\nb\nc\nd\ne", &[annotation(0..3, 8..9)])
        );

        assert!(output.contains("note"));
    }

    #[test]
    fn multiline_highlight_can_end_with_a_multibyte_character() {
        yansi::disable();
        let output = format!(
            "{}",
            display_annotations(None, "a\né", &[annotation(0..4, 0..4)])
        );

        assert!(output.contains("é"));
        assert!(output.contains("note"));
    }
}
