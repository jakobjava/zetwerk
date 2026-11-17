//! Tables as text: cells copied from a spreadsheet or a web page made
//! into a Typst table, a table written out as cells for a spreadsheet,
//! and the changes that shape a table from the page (a row or a column
//! more, one less, one moved, the first row made the header).
//!
//! Like `typst_text` and `figure_text`, all of it is text in and text
//! out: a change is the bytes to replace and what with, so that the rest
//! of the table stays as its author wrote it. Only tables `typst_text`
//! understands are changed ([`typst_text::table_at`]: cells written
//! between square brackets, a header of such cells, named arguments);
//! one with a cell over several columns, a line or a footer is not.

use std::fmt::Write as _;
use std::ops::Range;

use typst::syntax::{LinkedNode, Source, SyntaxKind};

use crate::typst_text::{self, Table};

/// A change to the text: the bytes to replace, and what with.
pub type Edit = (Range<usize>, String);

/// The most rows and columns cells pasted may have: more is no table
/// anyone types about.
const MOST: usize = 500;

/// The cells of `text` written with `delimiter` between them, as a
/// spreadsheet writes them: a row a line, a cell that holds the
/// delimiter, a line break or a quote between double quotes (a quote
/// in it doubled). Blank lines at the end are no rows.
#[must_use]
pub fn delimited(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    // Whether the cell began with a quote and it is not closed yet.
    let mut quoted = false;
    let mut began = true;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' if began => {
                quoted = true;
                began = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                began = true;
            }
            c if c == delimiter => {
                row.push(std::mem::take(&mut cell));
                began = true;
            }
            c => {
                cell.push(c);
                began = false;
            }
        }
    }
    if !began || !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    while rows
        .last()
        .is_some_and(|row| row.iter().all(|cell| cell.trim().is_empty()))
    {
        rows.pop();
    }
    rows
}

/// The cells of `text`, if it is cells: apart by tabs (as every
/// spreadsheet and every browser copies them), or, where `commas` says
/// the text was copied as such or it has two rows or more of as many
/// cells, by commas. `None` for text that is no table: one cell, rows
/// of other lengths, or more than a table anyone types.
#[must_use]
pub fn cells_in(text: &str, commas: bool) -> Option<Vec<Vec<String>>> {
    let even = |rows: &Vec<Vec<String>>| {
        let width = rows.first().map_or(0, Vec::len);
        // A spreadsheet leaves out what is empty at the end of a row.
        let fits = rows.iter().all(|row| (1..=width).contains(&row.len()));
        width >= 2 && fits && rows.len() <= MOST && width <= MOST
    };
    if text.contains('\t') {
        let rows = delimited(text, '\t');
        return even(&rows).then_some(rows);
    }
    let rows = delimited(text, ',');
    let width = rows.first().map_or(0, Vec::len);
    let same = rows.iter().all(|row| row.len() == width);
    ((commas || (rows.len() >= 2 && same)) && even(&rows)).then_some(rows)
}

/// What is gathered of a web page's table as its tokens come (see
/// [`cells_in_html`]).
#[derive(Default)]
struct Gathered {
    /// How deep in tables, and whether the first is over.
    depth: usize,
    done: bool,
    rows: Vec<Vec<String>>,
    cell: Option<String>,
    /// Where in its row the cell being read is.
    at: usize,
    /// In a `style` or `script`, whose text is no cell's.
    hidden: bool,
    refused: bool,
}

/// What takes the tokens.
struct Sink(std::cell::RefCell<Gathered>);

impl html5ever::tokenizer::TokenSink for Sink {
    type Handle = ();

    fn process_token(
        &self,
        token: html5ever::tokenizer::Token,
        _: u64,
    ) -> html5ever::tokenizer::TokenSinkResult<()> {
        use html5ever::tokenizer::{TagKind, Token, TokenSinkResult};
        let mut gathered = self.0.borrow_mut();
        let gathered = &mut *gathered;
        match token {
            Token::TagToken(tag) => {
                let start = tag.kind == TagKind::StartTag;
                let span = |name: &str| {
                    tag.attrs
                        .iter()
                        .find(|attr| &*attr.name.local == name)
                        .and_then(|attr| attr.value.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                };
                match (&*tag.name, start) {
                    ("style" | "script", start) => gathered.hidden = start,
                    _ if gathered.done => {}
                    ("table", true) => {
                        gathered.depth += 1;
                        gathered.refused |= gathered.depth > 1;
                    }
                    ("table", false) => {
                        gathered.depth = gathered.depth.saturating_sub(1);
                        gathered.done = gathered.depth == 0;
                    }
                    _ if gathered.depth != 1 => {}
                    ("tr", true) => gathered.rows.push(Vec::new()),
                    ("td" | "th", true) => {
                        if gathered.rows.is_empty() {
                            gathered.rows.push(Vec::new());
                        }
                        gathered.refused |= span("rowspan") > 1;
                        let columns = span("colspan").clamp(1, MOST);
                        if let Some(row) = gathered.rows.last_mut() {
                            gathered.at = row.len();
                            row.extend(std::iter::repeat_n(String::new(), columns));
                        }
                        gathered.cell = Some(String::new());
                    }
                    ("td" | "th", false) => {
                        let at = gathered.at;
                        if let (Some(cell), Some(row)) =
                            (gathered.cell.take(), gathered.rows.last_mut())
                            && let Some(place) = row.get_mut(at)
                        {
                            cell.trim().clone_into(place);
                        }
                    }
                    ("br", _) => {
                        if let Some(cell) = &mut gathered.cell {
                            cell.push('\n');
                        }
                    }
                    ("p" | "div", false) => {
                        if let Some(cell) = gathered.cell.as_mut().filter(|cell| !cell.is_empty()) {
                            cell.push('\n');
                        }
                    }
                    _ => {}
                }
            }
            Token::CharacterTokens(text) if !gathered.hidden && gathered.depth == 1 => {
                if let Some(cell) = &mut gathered.cell {
                    // Blanks in a page's text are one blank.
                    let mut blank = cell.ends_with([' ', '\n']) || cell.is_empty();
                    for c in text.chars() {
                        if c.is_whitespace() && c != '\u{a0}' {
                            if !blank {
                                cell.push(' ');
                            }
                            blank = true;
                        } else {
                            cell.push(if c == '\u{a0}' { ' ' } else { c });
                            blank = false;
                        }
                    }
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

/// What a web page's table (`text/html`, as a browser or a spreadsheet
/// copies one) holds, row by row: the text of each cell, a line break
/// in it kept, a cell over several columns followed by empty ones.
/// `None` where there is no table, it is in another, it has a cell over
/// several rows, or it is less than two cells.
#[must_use]
pub fn cells_in_html(html: &str) -> Option<Vec<Vec<String>>> {
    use html5ever::tendril::StrTendril;
    use html5ever::tokenizer::{BufferQueue, Tokenizer, TokenizerOpts};

    if !html.contains("<table") && !html.contains("<TABLE") {
        return None;
    }
    let queue = BufferQueue::default();
    queue.push_back(StrTendril::from_slice(html));
    let tokenizer = Tokenizer::new(
        Sink(std::cell::RefCell::default()),
        TokenizerOpts::default(),
    );
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    let gathered = tokenizer.sink.0.take();
    let mut rows: Vec<Vec<String>> = gathered
        .rows
        .into_iter()
        .filter(|row| !row.is_empty())
        .map(|row| {
            row.into_iter()
                .map(|cell| cell.trim_end().to_owned())
                .collect()
        })
        .collect();
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        row.resize(width, String::new());
    }
    let cells = width * rows.len();
    (!gathered.refused && cells >= 2 && width <= MOST && rows.len() <= MOST).then_some(rows)
}

/// Whether `cell` is a number, as a spreadsheet writes one: digits,
/// with a sign, a decimal point or comma, thousands apart, a percent
/// sign or a currency before or behind.
#[must_use]
pub fn is_number(cell: &str) -> bool {
    let trimmed = cell
        .trim()
        .trim_start_matches(['$', '€', '£', '¥', '+', '-', '−'])
        .trim_end_matches(['%', '$', '€', '£', '¥'])
        .trim();
    let mut digits = 0;
    let mut exponent = false;
    for (index, c) in trimmed.char_indices() {
        match c {
            '0'..='9' => digits += 1,
            '.' | ',' | '\'' | '\u{202f}' | '\u{a0}' | ' ' if digits > 0 => {}
            'e' | 'E' if digits > 0 && !exponent => {
                exponent = true;
                let rest = &trimmed[index + 1..];
                let rest = rest.trim_start_matches(['+', '-']);
                if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
            }
            '+' | '-' if exponent => {}
            _ => return false,
        }
    }
    digits > 0
}

/// `cell` written so that it shows as it is: what Typst's markup gives
/// a meaning to is escaped, a line break in it is one on the page.
#[must_use]
pub fn escaped(cell: &str) -> String {
    let lines: Vec<String> = cell
        .trim()
        .lines()
        .map(|line| escaped_line(line.trim()))
        .filter(|line| !line.is_empty())
        .collect();
    lines.join(" \\ ")
}

/// One line of a cell, escaped (see [`escaped`]).
fn escaped_line(line: &str) -> String {
    let mut written = String::with_capacity(line.len() + 8);
    let mut chars = line.chars().peekable();
    let mut first = true;
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        let escape = match c {
            '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '>' | '@' | '[' | ']' | '~' => true,
            // A comment.
            '/' => matches!(next, Some('/' | '*')) || (first && next == Some(' ')),
            // A dash, a soft hyphen; an item of a list, a heading.
            '-' => matches!(next, Some('-' | '?')) || (first && next.is_none_or(|n| n == ' ')),
            '+' | '=' => first && next.is_none_or(|n| n == ' ' || n == '='),
            // A number with a full stop behind it, before all else.
            '.' => {
                !written.is_empty()
                    && written.chars().all(|c| c.is_ascii_digit())
                    && next.is_none_or(char::is_whitespace)
            }
            _ => false,
        };
        if escape {
            written.push('\\');
        }
        written.push(c);
        first = false;
    }
    written
}

/// What a cell written as markup says, as text for a spreadsheet:
/// escapes undone, line breaks and blanks as one blank.
#[must_use]
pub fn plain(written: &str) -> String {
    let mut text = String::with_capacity(written.len());
    let mut chars = written.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek().copied()) {
            ('\\', Some(next)) if !next.is_whitespace() => {
                text.push(next);
                chars.next();
            }
            // A line break.
            ('\\', _) => text.push(' '),
            (c, _) if c.is_whitespace() => text.push(' '),
            (c, _) => text.push(c),
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A figure holding a table of `grid`, as the insert menu writes one
/// (see `typst_text::TEXT_INSERTS`): the text, and how far into it its
/// caption is, to be typed. A column of numbers is aligned to the right;
/// a first row of words above one is the header.
#[must_use]
pub fn new_table(grid: &[Vec<String>]) -> (String, usize) {
    let width = grid.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let filled = |cell: &String| !cell.trim().is_empty();
    // A column of numbers, below the first row if that is words.
    let numbers_below = |from: usize| -> Vec<bool> {
        (0..width)
            .map(|column| {
                let cells: Vec<&String> = grid
                    .iter()
                    .skip(from)
                    .filter_map(|row| row.get(column))
                    .filter(|cell| filled(cell))
                    .collect();
                !cells.is_empty() && cells.iter().all(|cell| is_number(cell))
            })
            .collect()
    };
    let header = grid.len() >= 2
        && grid[0].iter().any(filled)
        && grid[0].iter().all(|cell| !is_number(cell))
        && numbers_below(1).iter().any(|numbers| *numbers);
    let numbers = numbers_below(usize::from(header));
    let cells = |row: &Vec<String>| -> String {
        (0..width)
            .map(|column| format!("[{}]", escaped(row.get(column).map_or("", String::as_str))))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!("#figure(\n  table(\n    columns: {width},\n");
    if numbers.iter().any(|numbers| *numbers) {
        let align: Vec<&str> = numbers
            .iter()
            .map(|number| if *number { "right" } else { "left" })
            .collect();
        let _ = writeln!(text, "    align: ({}),", align.join(", "));
    }
    for (index, row) in grid.iter().enumerate() {
        if index == 0 && header {
            let _ = writeln!(text, "    table.header({}),", cells(row));
        } else {
            let _ = writeln!(text, "    {},", cells(row));
        }
    }
    text.push_str("  ),\n  caption: [");
    let caption = text.len();
    text.push_str("],\n)");
    (text, caption)
}

/// What is said of the arguments of a table that go column by column:
/// how many columns it has, written as a number or as a list of their
/// widths, and how each is aligned, where that is a list as long.
struct Columns {
    /// `table` or `grid`.
    name: String,
    count: Option<Range<usize>>,
    widths: Option<Vec<Range<usize>>>,
    align: Option<Vec<Range<usize>>>,
}

/// The items of the list `value` (a node of kind `Array`).
fn items(value: &LinkedNode) -> Vec<Range<usize>> {
    value
        .children()
        .filter(|part| {
            !matches!(
                part.kind(),
                SyntaxKind::LeftParen
                    | SyntaxKind::RightParen
                    | SyntaxKind::Comma
                    | SyntaxKind::Space
            )
        })
        .map(|part| part.range())
        .collect()
}

/// What `table` of `source` says of its columns.
fn columns_of(source: &Source, table: &Table) -> Option<Columns> {
    let text = source.text();
    let call =
        LinkedNode::new(source.root()).leaf_at(table.whole.start, typst::syntax::Side::After)?;
    let call = std::iter::successors(Some(call), |node| node.parent().cloned())
        .find(|node| node.kind() == SyntaxKind::FuncCall && node.range() == table.whole)?;
    let name = text.get(call.children().next()?.range())?.to_owned();
    let args = call
        .children()
        .find(|part| part.kind() == SyntaxKind::Args)?;
    let mut columns = Columns {
        name,
        count: None,
        widths: None,
        align: None,
    };
    for named in args
        .children()
        .filter(|arg| arg.kind() == SyntaxKind::Named)
    {
        let (Some(key), Some(value)) = (named.children().next(), named.children().next_back())
        else {
            continue;
        };
        match (text.get(key.range()), value.kind()) {
            (Some("columns"), SyntaxKind::Int) => columns.count = Some(value.range()),
            (Some("columns"), SyntaxKind::Array) => columns.widths = Some(items(&value)),
            (Some("align"), SyntaxKind::Array) => {
                let items = items(&value);
                if items.len() == table.columns {
                    columns.align = Some(items);
                }
            }
            _ => {}
        }
    }
    Some(columns)
}

/// The changes to a list whose items are `items` that put `with` in at
/// place `at` (behind the last, where that is how many it has).
fn item_added(items: &[Range<usize>], at: usize, with: &str) -> Option<Edit> {
    match items.get(at) {
        Some(item) => Some((item.start..item.start, format!("{with}, "))),
        None => items
            .last()
            .map(|last| (last.end..last.end, format!(", {with}"))),
    }
}

/// The changes that take item `at` out of a list of `items`. A list
/// left with one item keeps a comma behind it: else it is no list.
fn item_removed(items: &[Range<usize>], at: usize) -> Vec<Edit> {
    let mut edits = match (items.get(at), items.get(at + 1), at.checked_sub(1)) {
        (Some(item), Some(next), _) => vec![(item.start..next.start, String::new())],
        (Some(item), None, Some(before)) => vec![(items[before].end..item.end, String::new())],
        _ => return Vec::new(),
    };
    if items.len() == 2 {
        let left = &items[1 - at];
        edits.push((left.end..left.end, ",".to_owned()));
    }
    edits
}

/// The changes that swap items `a` and `b` of a list written in `text`.
fn items_swapped(text: &str, items: &[Range<usize>], a: usize, b: usize) -> Vec<Edit> {
    let (Some(first), Some(second)) = (items.get(a), items.get(b)) else {
        return Vec::new();
    };
    let (Some(first_text), Some(second_text)) = (text.get(first.clone()), text.get(second.clone()))
    else {
        return Vec::new();
    };
    vec![
        (first.clone(), second_text.to_owned()),
        (second.clone(), first_text.to_owned()),
    ]
}

/// What shapes a table: the changes to the text, and the cell the
/// cursor goes to in the table they leave, as its row and column.
#[derive(Debug, PartialEq, Eq)]
pub struct Shaped {
    pub edits: Vec<Edit>,
    pub cell: (usize, usize),
}

/// A table of a source, to be shaped.
pub struct Shape<'a> {
    source: &'a Source,
    pub table: Table,
    columns: Columns,
}

impl<'a> Shape<'a> {
    /// The table of `source` that begins at byte `start`, if it is one
    /// that can be shaped.
    #[must_use]
    pub fn new(source: &'a Source, start: usize) -> Option<Self> {
        let table = typst_text::table_at(source, start + 1).filter(|t| t.whole.start == start)?;
        let columns = columns_of(source, &table)?;
        Some(Self {
            source,
            table,
            columns,
        })
    }

    fn text(&self) -> &str {
        self.source.text()
    }

    /// How many rows it has, those of its header among them.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.table.cells.len().div_ceil(self.table.columns)
    }

    /// The row and the column of cell `index`.
    #[must_use]
    pub fn place(&self, index: usize) -> (usize, usize) {
        (index / self.table.columns, index % self.table.columns)
    }

    /// The cells of row `row`, as indices.
    fn row(&self, row: usize) -> Range<usize> {
        let columns = self.table.columns;
        (row * columns).min(self.table.cells.len())
            ..((row + 1) * columns).min(self.table.cells.len())
    }

    /// How many rows its header has.
    fn header_rows(&self) -> usize {
        self.table.header / self.table.columns
    }

    /// The byte behind row `row` and all that is written around it alone:
    /// behind the header where it is the header's last.
    fn row_end(&self, row: usize) -> usize {
        let cells = self.row(row);
        match &self.table.head {
            Some(head) if cells.end == self.table.header => head.end,
            _ => self.table.cells[cells.end - 1].end,
        }
    }

    /// What is written between two rows: a comma and the blanks behind
    /// it (a line break and the indent, as a table is usually written).
    fn between_rows(&self) -> String {
        let blanks = (0..self.rows().saturating_sub(1)).find_map(|row| {
            let next = self.table.cells.get(self.row(row + 1).start)?;
            let between = self.text().get(self.row_end(row)..next.start)?;
            let blanks = &between[between.rfind(',')? + 1..];
            blanks.trim().is_empty().then(|| blanks.to_owned())
        });
        format!(",{}", blanks.unwrap_or_else(|| " ".to_owned()))
    }

    /// What is written between two cells of a row.
    fn between_cells(&self) -> String {
        let cells = &self.table.cells;
        let blanks = (1..cells.len())
            .filter(|index| index % self.table.columns != 0)
            .find_map(|index| {
                let between = self.text().get(cells[index - 1].end..cells[index].start)?;
                let blanks = &between[between.rfind(',')? + 1..];
                (blanks.trim().is_empty() && !blanks.contains('\n')).then(|| blanks.to_owned())
            });
        format!(",{}", blanks.unwrap_or_else(|| " ".to_owned()))
    }

    /// A row of empty cells, as written.
    fn empty_row(&self) -> String {
        vec!["[]"; self.table.columns].join(&self.between_cells())
    }

    /// A row of empty cells above or below row `row`.
    #[must_use]
    pub fn row_added(&self, row: usize, below: bool) -> Option<Shaped> {
        let cells = self.row(row);
        let column = 0;
        if below {
            if row + 1 >= self.rows() {
                return self.rows_appended(1).map(|edits| Shaped {
                    edits,
                    cell: (row + 1, column),
                });
            }
            let at = if cells.end == self.table.header {
                self.row_end(row)
            } else {
                self.table.cells[cells.end - 1].end
            };
            let with = format!("{}{}", self.between_rows(), self.empty_row());
            return Some(Shaped {
                edits: vec![(at..at, with)],
                cell: (row + 1, column),
            });
        }
        let at = self.table.cells.get(cells.start)?.start;
        let with = format!("{}{}", self.empty_row(), self.between_rows());
        Some(Shaped {
            edits: vec![(at..at, with)],
            cell: (row, column),
        })
    }

    /// The changes that put `count` rows of empty cells behind the last,
    /// the last filled up first.
    fn rows_appended(&self, count: usize) -> Option<Vec<Edit>> {
        let last = self.rows().checked_sub(1)?;
        let at = self.row_end(last);
        let missing = self.row(last).len();
        let fill = ", []".repeat(self.table.columns - missing);
        let rows = format!("{}{}", self.between_rows(), self.empty_row()).repeat(count);
        Some(vec![(at..at, format!("{fill}{rows}"))])
    }

    /// The row of the cursor taken out.
    #[must_use]
    pub fn row_removed(&self, row: usize, column: usize) -> Option<Shaped> {
        let index = self.row(row).start;
        let edits = typst_text::table_without_row(&self.table, index);
        (!edits.is_empty()).then(|| Shaped {
            edits,
            cell: (row.min(self.rows().saturating_sub(2)), column),
        })
    }

    /// An empty column left or right of column `column`.
    #[must_use]
    pub fn column_added(&self, row: usize, column: usize, right: bool) -> Option<Shaped> {
        let at = column + usize::from(right);
        let mut edits = self.columns_put(at, 1)?;
        if let Some(widths) = &self.columns.widths {
            let like = self.text().get(widths.get(column)?.clone())?.to_owned();
            edits.extend(item_added(widths, at, &like));
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        Some(Shaped {
            edits,
            cell: (row, at),
        })
    }

    /// The changes that put `count` empty columns in at column `at` of
    /// every row (behind the last where `at` is as many as there are),
    /// saying they are there; from the last to the first.
    fn columns_put(&self, at: usize, count: usize) -> Option<Vec<Edit>> {
        let between = self.between_cells();
        let cells = &self.table.cells;
        let mut edits = Vec::new();
        for row in 0..self.rows() {
            let indices = self.row(row);
            let empties = vec!["[]"; count].join(&between);
            if at < indices.len() {
                let place = cells[indices.start + at].start;
                edits.push((place..place, format!("{empties}{between}")));
            } else if at == indices.len() {
                let place = cells[indices.end - 1].end;
                edits.push((place..place, format!("{between}{empties}")));
            }
        }
        let wide = self.table.columns + count;
        match (&self.columns.count, &self.columns.widths) {
            (Some(written), _) => edits.push((written.clone(), wide.to_string())),
            (None, Some(_)) => {}
            // A table that does not say how many columns it has, has one.
            (None, None) => return None,
        }
        if let Some(align) = &self.columns.align {
            for _ in 0..count {
                edits.extend(item_added(align, at, "auto"));
            }
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        Some(edits)
    }

    /// The column of the cursor taken out.
    #[must_use]
    pub fn column_removed(&self, row: usize, column: usize) -> Option<Shaped> {
        if self.table.columns < 2 {
            return None;
        }
        let cells = &self.table.cells;
        let mut edits = Vec::new();
        for each in 0..self.rows() {
            let indices = self.row(each);
            if column >= indices.len() {
                continue;
            }
            let index = indices.start + column;
            let range = if index + 1 < indices.end {
                cells[index].start..cells[index + 1].start
            } else if index > indices.start {
                cells[index - 1].end..cells[index].end
            } else {
                // The only cell of a last row that is not full.
                self.row_end(each - 1)..cells[index].end
            };
            edits.push((range, String::new()));
        }
        match (&self.columns.count, &self.columns.widths) {
            (Some(written), _) => {
                edits.push((written.clone(), (self.table.columns - 1).to_string()));
            }
            (None, Some(widths)) => edits.extend(item_removed(widths, column)),
            (None, None) => return None,
        }
        if let Some(align) = &self.columns.align {
            edits.extend(item_removed(align, column));
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        Some(Shaped {
            edits,
            cell: (row, column.min(self.table.columns - 2)),
        })
    }

    /// Row `row` moved down or up, past the next. `Err` says why not.
    pub fn row_moved(&self, row: usize, column: usize, down: bool) -> Result<Shaped, &'static str> {
        let other = if down { row + 1 } else { row.wrapping_sub(1) };
        if other >= self.rows() {
            return Err(if down {
                "it is the last row"
            } else {
                "it is the first row"
            });
        }
        let header = self.header_rows();
        if (row < header) != (other < header) {
            return Err(
                "the header stays above the rows (]h [h make the first row the header or not)",
            );
        }
        let (high, low) = (row.min(other), row.max(other));
        let span = |row: usize| {
            let cells = self.row(row);
            let full = cells.len() == self.table.columns;
            full.then(|| self.table.cells[cells.start].start..self.table.cells[cells.end - 1].end)
        };
        let (Some(first), Some(second)) = (span(high), span(low)) else {
            return Err("the last row is not full: fill it first");
        };
        let text = self.text();
        let (Some(first_text), Some(second_text)) =
            (text.get(first.clone()), text.get(second.clone()))
        else {
            return Err("it is not written as cells");
        };
        Ok(Shaped {
            edits: vec![
                (second, first_text.to_owned()),
                (first, second_text.to_owned()),
            ],
            cell: (other, column),
        })
    }

    /// Column `column` moved right or left, past the next.
    pub fn column_moved(
        &self,
        row: usize,
        column: usize,
        right: bool,
    ) -> Result<Shaped, &'static str> {
        let other = if right {
            column + 1
        } else {
            column.wrapping_sub(1)
        };
        if other >= self.table.columns {
            return Err(if right {
                "it is the last column"
            } else {
                "it is the first column"
            });
        }
        let text = self.text();
        let mut edits = Vec::new();
        for each in 0..self.rows() {
            let indices = self.row(each);
            let (a, b) = (indices.start + column, indices.start + other);
            if a.max(b) < indices.end {
                let cells = &self.table.cells;
                edits.extend(items_swapped(text, cells, a, b));
            }
        }
        if let Some(widths) = &self.columns.widths {
            edits.extend(items_swapped(text, widths, column, other));
        }
        if let Some(align) = &self.columns.align {
            edits.extend(items_swapped(text, align, column, other));
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        Ok(Shaped {
            edits,
            cell: (row, other),
        })
    }

    /// The first row made the header, or the header a row like the
    /// others.
    #[must_use]
    pub fn header_switched(&self, row: usize, column: usize) -> Option<Shaped> {
        let text = self.text();
        let cells = &self.table.cells;
        let edit = if let Some(head) = &self.table.head {
            let (first, last) = (cells.first()?, cells.get(self.table.header - 1)?);
            (head.clone(), text.get(first.start..last.end)?.to_owned())
        } else {
            let (first, last) = (cells.first()?, cells.get(self.table.columns - 1)?);
            let row = text.get(first.start..last.end)?;
            (
                first.start..last.end,
                format!("{}.header({row})", self.columns.name),
            )
        };
        Some(Shaped {
            edits: vec![edit],
            cell: (row, column),
        })
    }

    /// What the cells hold, as a spreadsheet takes them: apart by tabs,
    /// a row a line.
    #[must_use]
    pub fn as_cells(&self) -> String {
        let text = self.text();
        let mut written = String::new();
        for row in 0..self.rows() {
            let cells: Vec<String> = self.table.cells[self.row(row)]
                .iter()
                .map(|cell| {
                    let inner = text.get(cell.start + 1..cell.end - 1).unwrap_or("");
                    let plain = plain(inner);
                    // A cell with a tab or a quote is quoted.
                    if plain.contains(['\t', '"']) {
                        format!("\"{}\"", plain.replace('"', "\"\""))
                    } else {
                        plain
                    }
                })
                .collect();
            written.push_str(&cells.join("\t"));
            written.push('\n');
        }
        written
    }

    /// How many of the cells that `grid` would fill from the cell at
    /// `row` and `column` on hold something now.
    #[must_use]
    pub fn filled_under(&self, row: usize, column: usize, grid: &[Vec<String>]) -> usize {
        let text = self.text();
        let mut filled = 0;
        for (down, cells) in grid.iter().enumerate() {
            for across in 0..cells.len() {
                if column + across >= self.table.columns {
                    continue;
                }
                let index = (row + down) * self.table.columns + column + across;
                let Some(cell) = self.table.cells.get(index) else {
                    continue;
                };
                let inner = text.get(cell.start + 1..cell.end - 1).unwrap_or("");
                if !inner.trim().is_empty() {
                    filled += 1;
                }
            }
        }
        filled
    }
}

/// `text` after `edits`, each of which refers to the text the ones
/// before it left.
fn edited(text: &str, edits: &[Edit]) -> String {
    let mut text = text.to_owned();
    for (range, with) in edits {
        text.replace_range(range.clone(), with);
    }
    text
}

/// The table that begins at byte `start` of `source` with `grid` put
/// into it cell by cell from the cell at `row` and `column` on, made
/// larger where it does not fit: the bytes of the table, and what to
/// put in their place. `None` if it is no table that can be shaped.
#[must_use]
pub fn filled(
    source: &Source,
    start: usize,
    (row, column): (usize, usize),
    grid: &[Vec<String>],
) -> Option<(Range<usize>, String)> {
    let first = Shape::new(source, start)?;
    let whole = first.table.whole.clone();
    let wide = column + grid.iter().map(Vec::len).max().unwrap_or(0);
    let high = row + grid.len();
    let mut text = source.text().to_owned();
    if wide > first.table.columns {
        let edits = first.columns_put(first.table.columns, wide - first.table.columns)?;
        text = edited(&text, &edits);
    }
    let source = Source::detached(text.as_str());
    let shape = Shape::new(&source, start)?;
    if high > shape.rows() {
        let edits = shape.rows_appended(high - shape.rows())?;
        text = edited(&text, &edits);
    }
    let source = Source::detached(text.as_str());
    let shape = Shape::new(&source, start)?;
    let mut edits = Vec::new();
    for (down, cells) in grid.iter().enumerate() {
        for (across, cell) in cells.iter().enumerate() {
            let index = (row + down) * shape.table.columns + column + across;
            let place = shape.table.cells.get(index)?;
            edits.push((place.clone(), format!("[{}]", escaped(cell))));
        }
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let text = edited(&text, &edits);
    let source = Source::detached(text.as_str());
    let shape = Shape::new(&source, start)?;
    let new = text.get(shape.table.whole)?.to_owned();
    Some((whole, new))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    fn grid(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect()
    }

    #[test]
    fn cells_are_read_as_a_spreadsheet_writes_them() {
        type Case = (
            &'static str,
            bool,
            Option<&'static [&'static [&'static str]]>,
        );
        let cases: &[Case] = &[
            ("a\tb\nc\td\n", false, Some(&[&["a", "b"], &["c", "d"]])),
            // Windows line ends, an empty cell, the last row short.
            (
                "a\tb\r\n\td\r\ne\n\n",
                false,
                Some(&[&["a", "b"], &["", "d"], &["e"]]),
            ),
            // A cell with a tab, a line break and a quote in it.
            (
                "\"x\ty\"\t\"one\ntwo\"\n\"say \"\"hi\"\"\"\tz",
                false,
                Some(&[&["x\ty", "one\ntwo"], &["say \"hi\"", "z"]]),
            ),
            (
                "name,count\nfoo,2\n",
                false,
                Some(&[&["name", "count"], &["foo", "2"]]),
            ),
            (
                "a,\"b,c\"\n1,2\n",
                false,
                Some(&[&["a", "b,c"], &["1", "2"]]),
            ),
            // One line of words with commas is text, unless it was copied
            // as cells.
            ("Hello, world", false, None),
            ("Hello, world", true, Some(&[&["Hello", " world"]])),
            // Rows of other lengths, one cell, nothing.
            ("a,b\nc\n", false, None),
            ("a\tb\nc\td\te\n", false, None),
            ("just words\n", false, None),
            ("", false, None),
        ];
        for (text, commas, wanted) in cases {
            let wanted = wanted.map(grid);
            assert_eq!(cells_in(text, *commas), wanted, "{text:?}");
        }
    }

    #[test]
    fn numbers_are_known_as_such() {
        for number in [
            "1", "-2.5", "1,234.50", "1.234,50", "12 %", "$3", "4 €", "1e-3", "−7",
        ] {
            assert!(is_number(number), "{number}");
        }
        for word in [
            "", "a1", "1a", "-", "e5", "1e", "12-3-4x", "N/A", "3 apples",
        ] {
            assert!(!is_number(word), "{word}");
        }
    }

    #[test]
    fn a_new_table_is_written_as_the_insert_menu_writes_one() {
        let (text, caption) = new_table(&grid(&[&["Name", "Count"], &["a", "1"], &["b", "22"]]));
        assert_eq!(
            text,
            "#figure(\n  table(\n    columns: 2,\n    align: (left, right),\n    \
             table.header([Name], [Count]),\n    [a], [1],\n    [b], [22],\n  ),\n  caption: [],\n)"
        );
        assert_eq!(&text[caption..caption + 2], "],");
        // Words only: no header, no alignment; a short row filled up.
        let (text, _) = new_table(&grid(&[&["a", "b"], &["*c*"]]));
        assert_eq!(
            text,
            "#figure(\n  table(\n    columns: 2,\n    [a], [b],\n    [\\*c\\*], [],\n  ),\n  caption: [],\n)"
        );
    }

    /// What the page says, of all its text, in the order it is laid out.
    fn page_text(text: &str) -> String {
        use typst::layout::{Frame, FrameItem};
        fn walk(frame: &Frame, out: &mut String) {
            for (_, item) in frame.items() {
                match item {
                    FrameItem::Group(group) => walk(&group.frame, out),
                    FrameItem::Text(run) => out.push_str(&run.text),
                    _ => {}
                }
            }
        }
        let env = zetwerk_typst::Environment::new(zetwerk_typst::FontSet::Embedded);
        let project = zetwerk_typst::Project::in_memory(
            env,
            std::path::PathBuf::from("/"),
            "main.typ",
            text.to_owned(),
        )
        .unwrap();
        let compiled = zetwerk_typst::compile(&project.snapshot());
        let document = compiled
            .document
            .unwrap_or_else(|| panic!("{text}: {:?}", compiled.errors));
        let mut out = String::new();
        for page in document.pages() {
            walk(&page.frame, &mut out);
        }
        out
    }

    #[test]
    fn what_a_cell_holds_shows_as_it_is() {
        let cells = [
            "a*b*c",
            "x_1 and y_2",
            "#let x = 1",
            "$5 and $6",
            "[bracket] ]",
            "a // not a comment",
            "/* nor this */",
            "<label> @ref",
            "- not an item",
            "+ nor this",
            "= nor a heading",
            "1. nor a number",
            "/ nor: a term",
            "a ~ b",
            "back\\slash",
            "`raw`",
            "a--b",
            "maybe-?",
            "100 %",
        ];
        for cell in cells {
            let written = escaped(cell);
            let shown = page_text(&format!("#table(columns: 1, [{written}])"));
            let shown: String = shown.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(shown, cell, "{cell:?} written as {written:?}");
            // And back, for a spreadsheet.
            assert_eq!(plain(&written), cell);
        }
        // A line break in a cell is one on the page.
        assert_eq!(escaped("one\ntwo"), "one \\ two");
        assert_eq!(plain("one \\ two"), "one two");
    }

    const TABLE: &str =
        "Before.\n\n#table(\n  columns: 3,\n  [a], [b], [c],\n  [d], [e], [f],\n)\n\nAfter.\n";
    const HEADED: &str = "#table(\n  columns: (auto, 1fr),\n  align: (left, right),\n  table.header([N], [V]),\n  [x], [1],\n  [y], [2],\n)\n";

    /// `text` with the table in it shaped by `shape`, and the cell the
    /// cursor goes to, as the text of that cell.
    fn shaped(text: &str, shape: impl Fn(&Shape) -> Option<Shaped>) -> (String, String) {
        let source = Source::detached(text);
        let start = text.find("table(").unwrap();
        let table = Shape::new(&source, start).unwrap();
        let shaped = shape(&table).unwrap_or_else(|| panic!("{text}: shaped"));
        let after = edited(text, &shaped.edits);
        let source = Source::detached(after.as_str());
        let table = Shape::new(&source, start).unwrap_or_else(|| panic!("{after}: still a table"));
        let (row, column) = shaped.cell;
        let index = row * table.table.columns + column;
        let cell = table.table.cells[index].clone();
        (after.clone(), after[cell].to_owned())
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "a table of cases")]
    fn tables_are_shaped_cell_by_cell() {
        type Case = (
            &'static str,
            fn(&Shape) -> Option<Shaped>,
            &'static str,
            &'static str,
        );
        let cases: &[Case] = &[
            (
                TABLE,
                |t| t.row_added(0, true),
                "  [a], [b], [c],\n  [], [], [],\n  [d], [e], [f],\n)",
                "[]",
            ),
            (
                TABLE,
                |t| t.row_added(0, false),
                "  [], [], [],\n  [a], [b], [c],\n  [d], [e], [f],\n)",
                "[]",
            ),
            (
                TABLE,
                |t| t.row_added(1, true),
                "  [a], [b], [c],\n  [d], [e], [f],\n  [], [], [],\n)",
                "[]",
            ),
            (
                TABLE,
                |t| t.row_removed(0, 2),
                "columns: 3,\n  [d], [e], [f],\n)",
                "[f]",
            ),
            (
                TABLE,
                |t| t.column_added(1, 1, true),
                "columns: 4,\n  [a], [b], [], [c],\n  [d], [e], [], [f],\n)",
                "[]",
            ),
            (
                TABLE,
                |t| t.column_added(0, 0, false),
                "columns: 4,\n  [], [a], [b], [c],\n  [], [d], [e], [f],\n)",
                "[]",
            ),
            (
                TABLE,
                |t| t.column_added(0, 2, true),
                "columns: 4,\n  [a], [b], [c], [],\n  [d], [e], [f], [],\n)",
                "[]",
            ),
            (
                TABLE,
                |t| t.column_removed(1, 2),
                "columns: 2,\n  [a], [b],\n  [d], [e],\n)",
                "[e]",
            ),
            (
                TABLE,
                |t| t.column_removed(0, 0),
                "columns: 2,\n  [b], [c],\n  [e], [f],\n)",
                "[b]",
            ),
            (
                TABLE,
                |t| t.row_moved(0, 1, true).ok(),
                "  [d], [e], [f],\n  [a], [b], [c],\n)",
                "[b]",
            ),
            (
                TABLE,
                |t| t.column_moved(1, 0, true).ok(),
                "  [b], [a], [c],\n  [e], [d], [f],\n)",
                "[d]",
            ),
            (
                TABLE,
                |t| t.header_switched(1, 1),
                "  table.header([a], [b], [c]),\n  [d], [e], [f],\n)",
                "[e]",
            ),
            // With a header, widths and alignments as lists.
            (
                HEADED,
                |t| t.header_switched(0, 0),
                "  [N], [V],\n  [x], [1],",
                "[N]",
            ),
            (
                HEADED,
                |t| t.row_added(0, true),
                "  table.header([N], [V]),\n  [], [],\n  [x], [1],",
                "[]",
            ),
            (
                HEADED,
                |t| t.column_added(1, 0, true),
                "columns: (auto, auto, 1fr),\n  align: (left, auto, right),\n  \
                 table.header([N], [], [V]),\n  [x], [], [1],\n  [y], [], [2],",
                "[]",
            ),
            (
                HEADED,
                |t| t.column_removed(2, 0),
                "columns: (1fr,),\n  align: (right,),\n  table.header([V]),\n  [1],\n  [2],",
                "[2]",
            ),
            (
                HEADED,
                |t| t.column_moved(2, 1, false).ok(),
                "columns: (1fr, auto),\n  align: (right, left),\n  \
                 table.header([V], [N]),\n  [1], [x],\n  [2], [y],",
                "[2]",
            ),
            (
                HEADED,
                |t| t.row_moved(2, 0, false).ok(),
                "  table.header([N], [V]),\n  [y], [2],\n  [x], [1],",
                "[y]",
            ),
            // On one line.
            (
                "#table(columns: 2, [a], [b])",
                |t| t.row_added(0, true),
                "#table(columns: 2, [a], [b], [], [])",
                "[]",
            ),
        ];
        for (text, shape, wanted, cell) in cases {
            let (after, at) = shaped(text, shape);
            assert!(
                after.contains(wanted),
                "{text}\nbecame\n{after}\nnot with\n{wanted}"
            );
            assert_eq!(at, *cell, "{after}");
        }
    }

    #[test]
    fn what_cannot_be_done_to_a_table_is_refused() {
        let source = Source::detached(HEADED);
        let table = Shape::new(&source, 1).unwrap();
        assert!(table.row_moved(0, 0, false).is_err());
        assert!(
            table.row_moved(0, 0, true).is_err(),
            "the header stays first"
        );
        assert!(table.row_moved(2, 0, true).is_err());
        assert!(table.column_moved(0, 1, true).is_err());
        let one = Source::detached("#table(columns: 1, [a])");
        let table = Shape::new(&one, 1).unwrap();
        assert_eq!(table.column_removed(0, 0), None);
        assert_eq!(table.row_removed(0, 0), None);
        // A cell over two columns, a line: not a table to change.
        for text in [
            "#table(columns: 2, table.cell(colspan: 2)[a], [b], [c])",
            "#table(columns: 2, [a], [b], table.hline(), [c], [d])",
        ] {
            assert!(Shape::new(&Source::detached(text), 1).is_none(), "{text}");
        }
    }

    #[test]
    fn cells_are_put_into_a_table_that_grows_to_hold_them() {
        let source = Source::detached(TABLE);
        let start = TABLE.find("table(").unwrap();
        let pasted = grid(&[&["1", "2"], &["3", "4"], &["5"]]);
        let table = Shape::new(&source, start).unwrap();
        assert_eq!(table.filled_under(1, 1, &pasted), 2);
        let (range, new) = filled(&source, start, (1, 2), &pasted).unwrap();
        let mut after = TABLE.to_owned();
        after.replace_range(range, &new);
        assert_eq!(
            after,
            "Before.\n\n#table(\n  columns: 4,\n  [a], [b], [c], [],\n  [d], [e], [1], [2],\n  \
             [], [], [3], [4],\n  [], [], [5], [],\n)\n\nAfter.\n"
        );
    }

    #[test]
    fn a_table_is_written_out_for_a_spreadsheet() {
        let text = "#table(columns: 2, table.header([Name], [Say]), [\\#1], [a \\ b], [x\ty], [])";
        let source = Source::detached(text);
        let table = Shape::new(&source, 1).unwrap();
        assert_eq!(table.as_cells(), "Name\tSay\n#1\ta b\nx y\t\n");
        // And read back as it was.
        assert_eq!(
            cells_in(&table.as_cells(), false),
            Some(grid(&[&["Name", "Say"], &["#1", "a b"], &["x y", ""]]))
        );
    }

    #[test]
    fn a_table_of_a_web_page_is_read_cell_by_cell() {
        let html = "<meta charset=utf-8><style>td { color: red }</style><table>\
            <thead><tr><th>Name</th><th>Price &amp; tax</th></tr></thead>\
            <tbody><tr><td>A&nbsp;B</td><td>\n  1,50\n</td></tr>\
            <tr><td colspan=\"2\">both<br>lines</td></tr></tbody></table><table><tr><td>no</td></tr></table>";
        assert_eq!(
            cells_in_html(html),
            Some(grid(&[
                &["Name", "Price & tax"],
                &["A B", "1,50"],
                &["both\nlines", ""]
            ]))
        );
        assert_eq!(cells_in_html("<p>no table</p>"), None);
        assert_eq!(
            cells_in_html("<table><tr><td rowspan=2>a</td><td>b</td></tr></table>"),
            None
        );
        assert_eq!(
            cells_in_html("<table><tr><td><table><tr><td>a</td></tr></table></td></tr></table>"),
            None
        );
    }
}
