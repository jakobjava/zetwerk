//! The engine against Neovim.
//!
//! Random texts and random key sequences are run through headless Neovim
//! and through the engine, and buffer, cursor, mode and unnamed register
//! are compared. What the generators below produce is the part of Vim's
//! grammar the engine is held to; a disagreement is a bug in the engine.
//!
//! Skipped (with a note) on machines without `nvim`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a broken harness should fail loudly"
)]

use std::ops::Range;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use zetwerk_vim::keys;
use zetwerk_vim::{Edit, Engine, Host, Mode};

const ORACLE: &str = r#"
local cases = vim.json.decode(table.concat(vim.fn.readfile(arg[1]), "\n"))
vim.o.swapfile = false
vim.o.autoindent = false
vim.o.shiftwidth = 2
vim.o.softtabstop = 0
vim.o.smarttab = false
-- With the mode shown, Vim waits a second before overwriting a message.
vim.o.showmode = false
-- Without a screen, an error message makes the next command wait a second
-- for it to be read. An attached interface that takes the messages itself
-- avoids that.
vim.ui_attach(vim.api.nvim_create_namespace("zetwerk"), { ext_messages = true }, function() end)
vim.o.expandtab = true
-- `J` joins lines as they are. Neovim would take what it thinks are comment
-- leaders off them, by rules made for other languages: in Typst a leading
-- `-` is a list item.
-- Neovim maps a few keys by default (`Y` to `y$`, for one). Typed keys are
-- fed past mappings, but the keys of a macro are not.
vim.cmd("mapclear | mapclear!")
vim.o.formatoptions = ""
vim.o.wrap = false
vim.o.comments = ""
vim.o.textwidth = 0
vim.o.wrap = false
-- No pauses for messages nobody reads.
vim.o.more = false
vim.o.report = 99999
vim.o.shortmess = "aoOstTWAIcCqFS"
local results = {}
-- For finding a case that makes Neovim quit: `ZETWERK_ORACLE_LOG=file`
-- notes every case there before it is run.
local log = os.getenv("ZETWERK_ORACLE_LOG")
for index, case in ipairs(cases) do
  if log then
    local file = io.open(log, "a")
    file:write(vim.inspect(case.keys):gsub("\n", " "), "\n")
    file:close()
  end
  vim.cmd("enew!")
  vim.bo.swapfile = false
  -- Set the lines without recording it, so that undo cannot take them away.
  local levels = vim.bo.undolevels
  vim.bo.undolevels = -1
  vim.api.nvim_buf_set_lines(0, 0, -1, false, case.lines)
  vim.bo.undolevels = levels
  -- Nothing may carry over from the case before. The last change cannot
  -- be cleared, so it is set to one that changes nothing; the engine is
  -- started the same way.
  vim.fn.setcharsearch({ char = "" })
  -- `@@` plays the register played last, also in an earlier case. Playing
  -- an empty one leaves it with nothing to play.
  vim.fn.setreg("z", {})
  pcall(vim.cmd, "silent! normal! @z")
  vim.fn.setreg("/", "")
  vim.v.searchforward = 1
  -- (`silent`, or entering insert mode waits a second for a message left
  -- over from the case before to be read.)
  -- Marks may outlive the case that set them. (Before the insert below,
  -- which leaves the mark `gi` goes to.)
  pcall(vim.cmd, "delmarks!")
  pcall(vim.cmd, "delmarks <>")
  vim.cmd([[silent exe "normal! i\<Esc>"]])
  vim.api.nvim_win_set_cursor(0, { case.row, case.col })
  -- Where `''` goes back to is kept per window: start from here.
  vim.cmd("normal! m'")
  for _, name in ipairs({ '"', "0", "1", "2", "-", "a", "b", "q", "w" }) do vim.fn.setreg(name, {}) end
  for _, chunk in ipairs(case.keys) do
    local keys = vim.api.nvim_replace_termcodes(chunk, true, true, true)
    pcall(vim.api.nvim_feedkeys, keys, "ntx", false)
    -- Vim works out the column that vertical motions aim at lazily, when
    -- the screen is drawn. Nothing is drawn here, so ask for it, as a
    -- redraw between two commands would.
    vim.fn.getcurpos()
  end
  local cursor = vim.api.nvim_win_get_cursor(0)
  results[index] = {
    lines = vim.api.nvim_buf_get_lines(0, 0, -1, false),
    row = cursor[1], col = cursor[2],
    mode = vim.api.nvim_get_mode().mode,
    unnamed = vim.fn.getreg('"'), linewise = vim.fn.getregtype('"') == "V",
  }
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<C-\\><C-n>", true, false, true), "ntx", false)
end
vim.fn.writefile({ vim.json.encode(results) }, arg[2])
"#;

/// A text with snapshot undo: all the engine needs of a host.
#[derive(Default)]
struct Buffer {
    text: String,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

/// One undo step: the text on its other side, and the part of the text
/// with the step made that the step replaced (`None` until it replaced
/// something).
struct Step {
    other: String,
    changed: Option<Range<usize>>,
}

impl Host for Buffer {
    fn text(&self) -> &str {
        &self.text
    }

    fn edit(&mut self, edits: &[Edit], join: bool) {
        if !join || self.undo.is_empty() {
            self.undo.push(Step {
                other: self.text.clone(),
                changed: None,
            });
        }
        self.redo.clear();
        let step = self.undo.last_mut().unwrap();
        for edit in edits {
            let (start, end) = match &step.changed {
                None => (edit.range.start, edit.range.end),
                Some(changed) => (
                    changed.start.min(edit.range.start),
                    changed.end.max(edit.range.end),
                ),
            };
            step.changed = Some(start..end - edit.range.len() + edit.text.len());
            self.text.replace_range(edit.range.clone(), &edit.text);
        }
    }

    fn undo(&mut self) -> Option<Range<usize>> {
        let step = self.undo.pop()?;
        let changed = step.changed.clone().unwrap_or(0..0);
        let after = std::mem::replace(&mut self.text, step.other);
        let untouched = after.len() - changed.end;
        let restored = changed.start..self.text.len() - untouched;
        self.redo.push(Step {
            other: after,
            changed: step.changed,
        });
        Some(restored)
    }

    fn redo(&mut self) -> Option<Range<usize>> {
        let step = self.redo.pop()?;
        let before = std::mem::replace(&mut self.text, step.other);
        let changed = step.changed.clone().unwrap_or(0..0);
        self.undo.push(Step {
            other: before,
            changed: step.changed,
        });
        Some(changed)
    }
}

#[derive(Debug, Clone)]
struct Case {
    lines: Vec<String>,
    /// Line, counting from 1.
    row: usize,
    /// Byte within the line.
    col: usize,
    /// One entry per command. Between two commands Neovim gets to settle,
    /// as it would between two commands typed by hand.
    keys: Vec<String>,
}

#[derive(Debug, PartialEq)]
struct Outcome {
    lines: Vec<String>,
    row: usize,
    col: usize,
    mode: String,
    unnamed: String,
    linewise: bool,
}

fn run_engine(case: &Case) -> Outcome {
    let mut buffer = Buffer {
        text: case.lines.join("\n") + "\n",
        ..Buffer::default()
    };
    let mut engine = Engine::new();
    // See the oracle script: the last change is an insert of nothing.
    for press in keys::parse("i<Esc>").unwrap() {
        engine.key(press, &mut buffer);
    }
    let start: usize = case.lines[..case.row - 1]
        .iter()
        .map(|line| line.len() + 1)
        .sum();
    engine.set_cursor(start + case.col, &buffer.text);
    for press in keys::parse("m'").unwrap() {
        engine.key(press, &mut buffer);
    }
    for chunk in &case.keys {
        for press in keys::parse(chunk).unwrap() {
            engine.key(press, &mut buffer);
        }
        // Neovim leaves insert mode when it runs out of keys; do the same.
        if engine.mode() == Mode::Insert {
            engine.key(keys::parse("<Esc>").unwrap()[0], &mut buffer);
        }
    }
    let text = buffer.text.as_str();
    let body = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<String> = body.split('\n').map(str::to_owned).collect();
    let cursor = engine.cursor().min(body.len());
    let line_start = body[..cursor].rfind('\n').map_or(0, |at| at + 1);
    let (unnamed, linewise) = engine.register('"').unwrap_or(("", false));
    Outcome {
        row: body[..line_start].matches('\n').count() + 1,
        col: cursor - line_start,
        lines,
        mode: match engine.mode() {
            Mode::Normal | Mode::Insert => "n",
            Mode::Visual => "v",
            Mode::VisualLine => "V",
            Mode::Command => "c",
        }
        .to_owned(),
        unnamed: unnamed.to_owned(),
        linewise,
    }
}

/// Runs all `cases` through one Neovim. `None` if there is no Neovim.
fn run_neovim(cases: &[Case], name: &str) -> Option<Vec<Outcome>> {
    let directory: PathBuf =
        std::env::temp_dir().join(format!("zetwerk-oracle-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let (script, input, output) = (
        directory.join("oracle.lua"),
        directory.join("cases.json"),
        directory.join("out.json"),
    );
    std::fs::write(&script, ORACLE).unwrap();
    let encoded: Vec<Value> = cases
        .iter()
        .map(|case| json!({"lines": case.lines, "row": case.row, "col": case.col, "keys": case.keys}))
        .collect();
    std::fs::write(&input, serde_json::to_string(&encoded).unwrap()).unwrap();
    let status = Command::new("nvim")
        .args(["--headless", "-u", "NONE", "-i", "NONE", "-n", "-l"])
        .args([&script, &input, &output])
        .output();
    let Ok(status) = status else {
        let _ = std::fs::remove_dir_all(&directory);
        return None;
    };
    // Not `read_to_string`: Neovim can cut a character of several bytes in
    // half, and the case that makes it do so should show up as a
    // disagreement, not as unreadable results.
    let results = std::fs::read(&output).unwrap_or_else(|_| {
        panic!(
            "Neovim wrote no results ({}): {} {}",
            status.status,
            String::from_utf8_lossy(&status.stderr),
            String::from_utf8_lossy(&status.stdout)
        )
    });
    let _ = std::fs::remove_dir_all(&directory);
    let results: Vec<Value> = serde_json::from_str(&String::from_utf8_lossy(&results)).unwrap();
    Some(
        results
            .iter()
            .map(|result| Outcome {
                lines: result["lines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|line| line.as_str().unwrap().to_owned())
                    .collect(),
                row: result["row"].as_u64().unwrap() as usize,
                col: result["col"].as_u64().unwrap() as usize,
                mode: result["mode"].as_str().unwrap().to_owned(),
                unnamed: result["unnamed"].as_str().unwrap().to_owned(),
                linewise: result["linewise"].as_bool().unwrap(),
            })
            .collect(),
    )
}

/// A small xorshift generator, so that cases are the same on every run.
struct Random(u64);

impl Random {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

const WORDS: &[&str] = &[
    "foo", "bar", "a", "Baz", "x1", "é", "über", "_id", "if", "the", "word", "ab.cd", "(", ")",
    "(x)", "[y]", "{z}", "\"q\"", "'s'", ",", ".", ";", "-", "=", "a_b", "+1", "<t>", "Xy",
];

fn random_line(random: &mut Random) -> String {
    if random.chance(6) {
        return String::new();
    }
    let mut line = " ".repeat(if random.chance(4) {
        random.below(3) * 2
    } else {
        0
    });
    for index in 0..=random.below(7) {
        if index > 0 {
            line.push_str(if random.chance(5) {
                "  "
            } else if random.chance(6) {
                ""
            } else {
                " "
            });
        }
        line.push_str(random.pick(WORDS));
    }
    if random.chance(12) {
        line.push(' ');
    }
    line
}

fn random_case(random: &mut Random, keys: Vec<String>) -> Case {
    let lines: Vec<String> = (0..=random.below(6)).map(|_| random_line(random)).collect();
    let row = 1 + random.below(lines.len());
    let line = &lines[row - 1];
    let starts: Vec<usize> = line.char_indices().map(|(at, _)| at).collect();
    let col = if starts.is_empty() {
        0
    } else {
        starts[random.below(starts.len())]
    };
    Case {
        lines,
        row,
        col,
        keys,
    }
}

fn count(random: &mut Random) -> String {
    if random.chance(3) {
        (2 + random.below(3)).to_string()
    } else {
        String::new()
    }
}

const MOTIONS: &[&str] = &[
    "h", "l", "j", "k", "gj", "gk", "w", "W", "b", "B", "e", "E", "ge", "gE", "0", "^", "$", "gg",
    "G", "{", "}", "fa", "fo", "Fa", "t.", "T(", "f ", ";", ",", "%", "g_", "+", "-", "_", "|",
    "<Space>", "<BS>", "<CR>",
];

fn motion(random: &mut Random) -> String {
    let motion = random.pick(MOTIONS);
    // A count before `0` would make the `0` one of its digits.
    let count = if motion == "0" {
        String::new()
    } else {
        count(random)
    };
    format!("{count}{motion}")
}

const OBJECTS: &[&str] = &[
    "iw", "aw", "iW", "aW", "ip", "ap", "i(", "a(", "i)", "ib", "i[", "a]", "i{", "aB", "i<lt>",
    "a>", "i\"", "a\"", "i'", "a'",
];

/// Text typed in insert mode. If the command that should have started the
/// insert fails, Vim runs these keys as commands, so they only use keys the
/// engine knows.
const TYPED: &[&str] = &["x", "ab", "a b", "é", "", "X.y", "word two"];

fn operation(random: &mut Random) -> String {
    // `<` opens a key name in the notation, so the operator is `<lt>`.
    let operator = random.pick(&["d", "c", "y", ">", "<lt>", "gu", "gU", "g~"]);
    let register = if random.chance(6) {
        random.pick(&["\"a", "\"b", "\"A", "\"_"])
    } else {
        ""
    };
    let prefix = count(random);
    let target = match random.below(8) {
        0 => {
            // Doubled: `dd`, `guu`, `<<`.
            let last = if operator == "<lt>" {
                operator
            } else {
                &operator[operator.len() - 1..]
            };
            format!("{}{last}", count(random))
        }
        1 | 2 => format!("{}{}", count(random), random.pick(OBJECTS)),
        // Not `g_`: from a blank after a character of several bytes it
        // leaves Neovim's cursor inside that character, and the operator
        // then works on half of it.
        // Nor Space and Backspace, which with an operator go on into the
        // next line by rules of their own that the engine does not have:
        // there they stay in their line.
        _ => loop {
            let motion = motion(random);
            if !motion.ends_with("g_") && !motion.ends_with("<Space>") && !motion.ends_with("<BS>")
            {
                break motion;
            }
        },
    };
    let typed = if operator == "c" {
        format!("{}<Esc>", random.pick(TYPED))
    } else {
        String::new()
    };
    format!("{register}{prefix}{operator}{target}{typed}")
}

fn command(random: &mut Random) -> String {
    let typed = |random: &mut Random| format!("{}<Esc>", random.pick(TYPED));
    match random.below(30) {
        0 => format!("{}x", count(random)),
        1 => format!("{}X", count(random)),
        2 => format!("{}D", count(random)),
        3 => format!("C{}", typed(random)),
        4 => format!("{}s{}", count(random), typed(random)),
        5 => format!("S{}", typed(random)),
        6 => format!("{}Y", count(random)),
        7 => format!("{}p", count(random)),
        8 => format!("{}P", count(random)),
        9 => format!("{}J", count(random)),
        10 => format!("{}r{}", count(random), random.pick(&["x", "é", " ", "."])),
        11 => format!("{}~", count(random)),
        12 => format!("{}i{}", count(random), typed(random)),
        13 => format!("{}a{}", count(random), typed(random)),
        14 => format!("I{}", typed(random)),
        15 => format!("A{}", typed(random)),
        16 => format!("{}o{}", count(random), typed(random)),
        17 => format!("{}O{}", count(random), typed(random)),
        18 => "u".to_owned(),
        19 => "<C-r>".to_owned(),
        20 | 21 => format!("{}.", count(random)),
        22 => format!("{}gJ", count(random)),
        25 => format!("{}R{}", count(random), typed(random)),
        // Backspace only over what was typed: going back further, Neovim
        // changes characters it did not replace, in ways the engine does
        // not follow. There it only moves back.
        26 => format!(
            "R{}<BS><BS>{}",
            random.pick(&["ab", "a b", "X.y", "word two"]),
            typed(random)
        ),
        27 => format!("R{}<CR>{}", random.pick(TYPED), typed(random)),
        // Cursor keys in insert mode end one insert and begin another.
        23 => format!(
            "{}{}{}{}{}",
            count(random),
            random.pick(&["i", "a", "A", "o"]),
            random.pick(TYPED),
            random.pick(&["<Left>", "<Right>", "<Up>", "<Down>", "<Home>", "<End>"]),
            typed(random)
        ),
        24 => format!(
            "{}i{}<C-u>{}<CR>{}",
            count(random),
            random.pick(TYPED),
            random.pick(TYPED),
            typed(random)
        ),
        _ => format!(
            "i{}<BS><BS>{}<C-w>z<Esc>",
            random.pick(TYPED),
            random.pick(TYPED)
        ),
    }
}

fn visual(random: &mut Random) -> Vec<String> {
    // Not compared yet: text objects in visual mode (Vim extends the
    // selection in ways the engine does not model), putting over a
    // selection, and `I` and `A`, which the engine has in a simple form
    // that agrees with Neovim in most cases but not in all.
    let mut keys = vec![random.pick(&["v", "V"]).to_owned()];
    for _ in 0..=random.below(3) {
        keys.push(motion(random));
    }
    if random.chance(8) {
        keys.push("o".to_owned());
    }
    let end = random.pick(&[
        "d", "y", "x", ">", "<lt>", "~", "u", "U", "J", "c", "rx", "<Esc>", "", "D", "Y", "X", "s",
        "S", "C", "gJ", "v", "V",
    ]);
    let mut last = end.to_owned();
    if matches!(end, "c" | "s" | "S" | "C") {
        last.push_str(random.pick(TYPED));
        last.push_str("<Esc>");
    }
    keys.push(last);
    keys
}

/// Runs `count` cases built by `keys` and reports every disagreement.
///
/// `ZETWERK_ORACLE_ROUNDS=n` runs n times as many cases, on other texts and
/// keys each round: for a long look after changes to the engine.
fn compare(name: &str, seed: u64, count: usize, keys: impl FnMut(&mut Random) -> Vec<String>) {
    let rounds = std::env::var("ZETWERK_ORACLE_ROUNDS")
        .ok()
        .and_then(|rounds| rounds.parse().ok())
        .unwrap_or(1_u64);
    let mut keys = keys;
    for round in 0..rounds {
        let seed = seed.wrapping_add(round.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1;
        compare_round(name, seed, count, &mut keys);
    }
}

fn compare_round(
    name: &str,
    seed: u64,
    count: usize,
    keys: &mut impl FnMut(&mut Random) -> Vec<String>,
) {
    let mut random = Random(seed);
    let cases: Vec<Case> = (0..count)
        .map(|_| {
            let keys = keys(&mut random);
            random_case(&mut random, keys)
        })
        .collect();
    let Some(expected) = run_neovim(&cases, name) else {
        eprintln!("nvim is not installed; skipping the comparison with Neovim");
        return;
    };
    assert_eq!(expected.len(), cases.len(), "Neovim stopped early");
    let mut wrong = 0;
    for (case, expected) in cases.iter().zip(&expected) {
        let caught = std::panic::catch_unwind(|| run_engine(case));
        let Ok(actual) = caught else {
            panic!(
                "{name}: the engine panicked on keys {:?} at {}:{} in {:?}",
                case.keys, case.row, case.col, case.lines
            );
        };
        if actual != *expected {
            wrong += 1;
            if wrong <= 12 {
                eprintln!(
                    "\n{name}: keys {:?} at {}:{} in {:?}\n  neovim: {expected:?}\n  engine: {actual:?}",
                    case.keys, case.row, case.col, case.lines
                );
            }
        }
    }
    assert!(
        wrong == 0,
        "{name}: {wrong} of {count} cases disagree with Neovim"
    );
}

#[test]
fn motions() {
    compare("motions", 0x9e37_79b9_7f4a_7c15, 1500, |random| {
        (0..=random.below(3)).map(|_| motion(random)).collect()
    });
}

#[test]
fn operators() {
    compare("operators", 0xc2b2_ae3d_27d4_eb4f, 2500, |random| {
        vec![operation(random)]
    });
}

#[test]
fn commands() {
    compare("commands", 0x1656_67b1_9e37_79f9, 2000, |random| {
        // A yank or delete first, so that puts have something to put.
        let setup = if random.chance(2) {
            vec![operation(random)]
        } else {
            Vec::new()
        };
        setup.into_iter().chain([command(random)]).collect()
    });
}

#[test]
fn visual_mode() {
    compare("visual", 0x27d4_eb2f_1656_67c5, 2000, visual);
}

/// A selection and what is done to it, for use among other commands: ends
/// in normal mode, and without `$` and `g_`. Those make the selection one
/// "to the end of the line", and repeating a change to such a selection
/// overflows a number in Neovim, which then takes the text before the
/// cursor instead of the text after it. The engine takes the rest of the
/// line.
fn closed_visual(random: &mut Random) -> Vec<String> {
    let mut keys = visual(random);
    keys.retain(|chunk| !chunk.ends_with('$') && !chunk.ends_with("g_"));
    if matches!(keys.last().map(String::as_str), Some("" | "v" | "V")) {
        keys.push("<Esc>".to_owned());
    }
    keys
}

#[test]
fn sequences() {
    compare("sequences", 0x85eb_ca6b_c2b2_ae35, 4000, |random| {
        let mut selected = false;
        (0..2 + random.below(5))
            .flat_map(|_| match random.below(10) {
                0..=2 => vec![motion(random)],
                3..=5 => vec![operation(random)],
                6 => {
                    selected = true;
                    closed_visual(random)
                }
                _ => {
                    let command = command(random);
                    // Here a selection may have been made while the cursor
                    // was aiming at the end of its line for other reasons,
                    // which runs into the overflow described above. `.`
                    // after a selection is compared in `visual_repeat`.
                    if selected && command.ends_with('.') {
                        vec![motion(random)]
                    } else {
                        vec![command]
                    }
                }
            })
            .collect()
    });
}

/// Text to search for: nothing that means something in a Vim pattern,
/// which the engine does not have. It looks for the text as it is typed.
const PATTERNS: &[&str] = &[
    "foo", "bar", "a", "the", "if", "x1", "Baz", "word", "é", "id", "b", "o", "ab", "aa", "nope",
];

fn search(random: &mut Random) -> String {
    let pattern = |random: &mut Random| random.pick(PATTERNS);
    match random.below(10) {
        0..=2 => format!("{}/{}<CR>", count(random), pattern(random)),
        3 => format!("{}?{}<CR>", count(random), pattern(random)),
        4 => format!("{}n", count(random)),
        5 => format!("{}N", count(random)),
        6 => format!("{}*", count(random)),
        7 => format!("{}#", count(random)),
        // Searches as what an operator works up to.
        8 => format!(
            "{}{}{}<CR>",
            random.pick(&["d", "y", "gU"]),
            random.pick(&["/", "?"]),
            pattern(random)
        ),
        _ => format!(
            "{}{}",
            random.pick(&["d", "y", "g~"]),
            random.pick(&["n", "N"])
        ),
    }
}

#[test]
fn searches() {
    compare("search", 0x6a09_e667_f3bc_c909, 3000, |random| {
        (0..2 + random.below(4))
            .map(|_| match random.below(10) {
                0..=5 => search(random),
                6..=8 => motion(random),
                _ => ".".to_owned(),
            })
            .collect()
    });
}

/// Marks, and the place the cursor last jumped from. Only up to the first
/// change of the text: the engine's marks stay with the text they were put
/// on, Vim's stay at their line and column.
#[test]
fn marks() {
    compare("marks", 0x3c6e_f372_fe94_f82b, 3000, |random| {
        let mark = |random: &mut Random| random.pick(&["a", "b"]);
        let mut keys: Vec<String> = (0..3 + random.below(6))
            .map(|_| match random.below(20) {
                0..=6 => motion(random),
                7 => format!("/{}<CR>", random.pick(PATTERNS)),
                8 => random.pick(&["n", "N", "*", "#"]).to_owned(),
                9..=11 => format!("m{}", mark(random)),
                12..=14 => format!("{}{}", random.pick(&["`", "'"]), mark(random)),
                15 | 16 => random.pick(&["``", "''"]).to_owned(),
                17 => format!(
                    "y{}{}",
                    random.pick(&["`", "'"]),
                    random.pick(&["a", "b", "'"])
                ),
                18 => format!("{}{}<Esc>", random.pick(&["v", "V"]), motion(random)),
                _ => format!("gv{}<Esc>", motion(random)),
            })
            .collect();
        keys.push(match random.below(6) {
            0 => format!(
                "d{}{}",
                random.pick(&["`", "'"]),
                random.pick(&["a", "b", "'"])
            ),
            1 => format!("c`{}X<Esc>", mark(random)),
            2 => format!(">'{}", mark(random)),
            3 => "gvd".to_owned(),
            4 => format!("gi{}<Esc>", random.pick(TYPED)),
            _ => "``".to_owned(),
        });
        keys
    });
}

/// Where the engine means to differ from Vim: a mark stays with the text it
/// was put on, whatever is typed or deleted before it, and comes back
/// with an undo.
#[test]
fn marks_stay_with_their_text() {
    let mut buffer = Buffer {
        text: "one two three\nfour\n".to_owned(),
        ..Buffer::default()
    };
    let mut engine = Engine::new();
    let press = |engine: &mut Engine, buffer: &mut Buffer, notation: &str| {
        for press in keys::parse(notation).unwrap() {
            engine.key(press, buffer);
        }
    };
    // Mark "three", then change the text before it in several ways.
    press(
        &mut engine,
        &mut buffer,
        "2wma0iXX <Esc>ggOnew line<Esc>Gdd",
    );
    assert_eq!(buffer.text, "new line\nXX one two three\n");
    press(&mut engine, &mut buffer, "gg`a");
    assert_eq!(&buffer.text[engine.cursor()..], "three\n");
    press(&mut engine, &mut buffer, "uuugg`a");
    assert_eq!(buffer.text, "one two three\nfour\n");
    assert_eq!(&buffer.text[engine.cursor()..], "three\nfour\n");
    // Text with a mark in it goes: the mark is where the text was.
    press(&mut engine, &mut buffer, "0dwgg`a");
    assert_eq!(&buffer.text[engine.cursor()..], "three\nfour\n");
    press(&mut engine, &mut buffer, "0d$G`a");
    assert_eq!(engine.cursor(), 0);
}

/// Records a macro of a few commands and plays it, also more often than
/// it can succeed: a macro ends at the first command that fails.
#[test]
fn macros() {
    compare("macros", 0xbb67_ae85_84ca_a73b, 2000, |random| {
        let register = random.pick(&["q", "w"]);
        let mut keys = vec![format!("q{register}")];
        for _ in 0..=random.below(3) {
            keys.push(match random.below(4) {
                0 => motion(random),
                1 => operation(random),
                // Not `~`, `p` and `P`: once `.` has repeated one of them
                // inside a macro, Neovim drops the rest of the macro.
                _ => loop {
                    let command = command(random);
                    let key = command.trim_start_matches(|c: char| c.is_ascii_digit());
                    if !matches!(key, "~" | "p" | "P") {
                        break command;
                    }
                },
            });
        }
        keys.push("q".to_owned());
        for _ in 0..=random.below(2) {
            keys.push(match random.below(5) {
                0 => motion(random),
                1 => format!("{}@@", count(random)),
                _ => format!("{}@{register}", count(random)),
            });
        }
        keys
    });
}

#[test]
fn visual_repeat() {
    compare("repeat", 0x1b87_3593_cc9e_2d51, 2000, |random| {
        let mut keys = closed_visual(random);
        for _ in 0..=random.below(2) {
            for _ in 0..random.below(3) {
                keys.push(motion(random));
            }
            keys.push(".".to_owned());
        }
        if random.chance(3) {
            keys.push("u".to_owned());
        }
        keys
    });
}

/// For looking into one case by hand:
/// (`§` separates commands too, for when the keys have a `~~` in them.)
/// `ZETWERK_PROBE='keys~~keys|row|col|line|line' cargo test -p zetwerk-vim --test oracle probe -- --ignored --nocapture`
#[test]
#[ignore = "a tool, not a test"]
fn probe() {
    let Ok(spec) = std::env::var("ZETWERK_PROBE") else {
        return;
    };
    let mut parts = spec.split('|');
    let separator = if spec.contains('§') { "§" } else { "~~" };
    let keys: Vec<String> = parts
        .next()
        .unwrap()
        .split(separator)
        .map(str::to_owned)
        .collect();
    let row = parts.next().unwrap().parse().unwrap();
    let col = parts.next().unwrap().parse().unwrap();
    let lines: Vec<String> = parts.map(str::to_owned).collect();
    for upto in 1..=keys.len() {
        let case = Case {
            lines: lines.clone(),
            row,
            col,
            keys: keys[..upto].to_vec(),
        };
        let neovim = run_neovim(std::slice::from_ref(&case), "probe").map(|mut all| all.remove(0));
        let engine = run_engine(&case);
        let mark = if neovim.as_ref() == Some(&engine) {
            "  "
        } else {
            "!!"
        };
        println!(
            "{mark} after {:?}\n     neovim: {neovim:?}\n     engine: {engine:?}",
            keys[upto - 1]
        );
    }
}

/// Runs `cases` (each the keys of one case, apart by commands) on
/// `lines`, the cursor at the start, through Neovim and the engine, and
/// says where the two differ.
fn compare_fixed(name: &str, lines: &[&str], cases: &[&[&str]]) {
    let lines: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    let cases: Vec<Case> = cases
        .iter()
        .map(|keys| Case {
            lines: lines.clone(),
            row: 1,
            col: 0,
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
        })
        .collect();
    let Some(expected) = run_neovim(&cases, name) else {
        eprintln!("nvim is not installed; skipping the comparison with Neovim");
        return;
    };
    let wrong: Vec<String> = cases
        .iter()
        .zip(&expected)
        .filter(|(case, expected)| run_engine(case) != **expected)
        .map(|(case, expected)| {
            format!(
                "{:?}\n  neovim: {expected:?}\n  engine: {:?}",
                case.keys,
                run_engine(case)
            )
        })
        .collect();
    assert!(wrong.is_empty(), "{name}:\n{}", wrong.join("\n"));
}

/// Ctrl-O back through the places jumped from, Ctrl-I and Tab on again:
/// each jump from a line of its own, as Vim keeps one place a line. Not
/// back past the first: the script that starts Neovim jumped before.
#[test]
fn jump_list() {
    compare_fixed(
        "jump_list",
        &["one two", "three", "four five", "six", "seven"],
        &[
            &["3G", "5G", "<C-o>"],
            &["3G", "5G", "<C-o>", "<C-o>"],
            &["3G", "5G", "2<C-o>"],
            &["3G", "5G", "<C-o>", "<C-o>", "<C-i>"],
            &["3G", "5G", "<C-o>", "<C-o>", "<Tab>"],
            &["3G", "5G", "<C-o>", "<C-o>", "2<C-i>"],
            &["3G", "5G", "<C-o>", "<C-i>", "<C-i>"],
            &["w", "G", "<C-o>"],
            &["/four<CR>", "gg", "<C-o>"],
            &["/six<CR>", "w", "<C-o>", "<C-i>"],
            &["<Tab>"],
        ],
    );
}

/// Ctrl-O while typing: one command of normal mode, and typing goes on.
#[test]
fn insert_ctrl_o() {
    compare_fixed(
        "insert_ctrl_o",
        &["one two", "three"],
        &[
            &["iX<C-o>xY"],
            &["AX<C-o>0Y"],
            &["AX<C-o>xY"],
            &["wiX<C-o>dwY"],
            &["iX<C-o>jY"],
            &["AX<C-o>~Y"],
            &["iX<C-o>3lY"],
            &["iX<C-o>j0Y"],
            &["AX<C-o>$Y"],
            &["iX<C-o><Esc>Y"],
            &["iX<C-o>xY", "u"],
            &["iX<C-o>xY", "j."],
        ],
    );
}
