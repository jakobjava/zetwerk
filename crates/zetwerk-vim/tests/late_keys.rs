//! The keys of Vim that came late, against Neovim: the change list,
//! Ctrl-A and Ctrl-X, `@:`, the command-line history, `:normal`, `:g`,
//! `:v`, `:sort`, Ctrl-6, `gi`, `gv`, `o` in a selection, `.` with a
//! count. The harness is that of `oracle.rs`.
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

#[test]
fn change_list() {
    compare_fixed(
        "change_list",
        &["one two", "three four", "five six", "seven"],
        &[
            &["x", "jjwx", "g;"],
            &["x", "jjwx", "g;", "g;"],
            &["x", "jjwx", "g;", "g;", "g;"],
            &["x", "jjwx", "g;", "g;", "g,"],
            &["x", "jjwx", "G", "2g;"],
            &["wx", "wx", " g;"],
            &["jdd", "g;"],
            &["Afoo<Esc>", "G", "g;"],
            &["x", "jjwx", "Gx", "g;", "g;", "g,", "g,"],
            &["g;"],
        ],
    );
}

#[test]
fn numbers_up_and_down() {
    compare_fixed(
        "numbers",
        &[
            "x 7 y", "-3 a 4", "0x0f", "abc9", "007 9", "a-1", "0xFE z", "9", "x0b101",
        ],
        &[
            &["<C-a>"],
            &["5<C-a>"],
            &["j<C-a>"],
            &["j3<C-x>"],
            &["j$<C-a>"],
            &["2j<C-a>"],
            &["2j5<C-a>"],
            &["3j<C-x>"],
            &["4j<C-a>"],
            &["4j10<C-x>"],
            &["5j<C-a>"],
            &["6j<C-a>"],
            &["7j<C-a>", "."],
            &["7j<C-a>", "3."],
            &["8j<C-a>"],
            &["$<C-a>"],
            &["jw<C-x>"],
            &["<C-a>", "u"],
        ],
    );
}

#[test]
fn command_lines_again() {
    compare_fixed(
        "command_lines",
        &["one", "two", "three", "four"],
        &[
            &[":normal Ax<CR>", "j@:"],
            &[":normal Ax<CR>", "j@:", "j@@"],
            &[":normal Ax<CR>", "j2@:"],
            &[":normal Ax<CR>", ":normal j<CR>", ":<Up><CR>"],
            &[":normal Ax<CR>", ":normal Ay<CR>", "j:<Up><Up><CR>"],
            &[":normal Ax<CR>", ":normal j<CR>", "j:normal A<Up><CR>"],
            &[":normal Ax<CR>", ":normal j<CR>", ":<C-p><C-p><CR>"],
            &[":normal Ax<CR>", ":normal j<CR>", ":<C-p><C-p><C-n><CR>"],
            &["w:normal A<C-r><C-w><CR>"],
            &["yiw:normal A<C-r>\"<CR>"],
        ],
    );
}

#[test]
fn normal_on_lines() {
    compare_fixed(
        "normal",
        &["one two", "three", "four five", "six"],
        &[
            &[":normal Ax<CR>"],
            &[":%normal Ax<CR>"],
            &[":2,3normal dw<CR>"],
            &[":%normal Ax<CR>", "u"],
            &[":%normal Ax<CR>", "j."],
            &[":normal ix<CR>"],
            &[":normal d<CR>"],
            &[":%norm! x<CR>"],
            &[":%normal fexx<CR>"],
        ],
    );
}

#[test]
fn global_commands() {
    compare_fixed(
        "global",
        &["one two", "three", "four five", "six", "seven", "one again"],
        &[
            &[":g/e/d<CR>"],
            &[":v/e/d<CR>"],
            &[":g!/e/d<CR>"],
            &[":g/^.....$/d<CR>"],
            &[":g/o/normal Ax<CR>"],
            &[":g/e/d<CR>", "u"],
            &[":g/\\<lt>one\\>/d<CR>"],
            &[":2,4g/e/d<CR>"],
            &[":g/o/normal dd<CR>"],
            &[":g/xyz/d<CR>"],
            &["/fo<CR>", ":g//d<CR>"],
            &[":g/e\\|x/d<CR>"],
        ],
    );
}

#[test]
fn sorting_lines() {
    compare_fixed(
        "sort",
        &["b 10", "a 9", "B 2", "a 9", "c", "A 1"],
        &[
            &[":sort<CR>"],
            &[":sort!<CR>"],
            &[":sort i<CR>"],
            &[":sort u<CR>"],
            &[":sort n<CR>"],
            &[":sort! n<CR>"],
            &[":2,4sort<CR>"],
            &[":sort<CR>", "u"],
        ],
    );
}

#[test]
fn small_keys_already_there() {
    compare_fixed(
        "small_keys",
        &["one two", "three four", "five"],
        &[
            &["wiX<Esc>", "j0", "giY<Esc>"],
            &["Afoo<Esc>", "jgiY<Esc>"],
            &["vjl<Esc>", "gg", "gv", "d"],
            &["vjl", "o", "d"],
            &["Vj", "o", "d"],
            &["x", "3."],
            &["dw", "j2."],
            &["3x", "j."],
        ],
    );
}

#[test]
fn block_selections() {
    compare_fixed(
        "block",
        &["abcdef", "ab", "abcdefgh", "xyz"],
        &[
            &["l<C-v>jjld"],
            &["l<C-v>jjly"],
            &["l<C-v>jjl$d"],
            &["2l<C-v>jjd"],
            &["l<C-v>jjlcXY<Esc>"],
            &["3l<C-v>jjcXY<Esc>"],
            &["l<C-v>jjIXY<Esc>"],
            &["3l<C-v>jjIXY<Esc>"],
            &["l<C-v>jjlAXY<Esc>"],
            &["3l<C-v>jjAXY<Esc>"],
            &["l<C-v>jj$AXY<Esc>"],
            &["l<C-v>jjly", "G$p"],
            &["l<C-v>jjly", "Gp"],
            &["l<C-v>jjly", "gg$P"],
            &["l<C-v>jjly", "ggp"],
            &["l<C-v>jjly", "jjp"],
            &["l<C-v>jjly", "3p"],
            &["l<C-v>jjlOd"],
            &["l<C-v>jjlod"],
            &["l<C-v>jjlx", "u"],
            &["l<C-v>jjl~"],
            &["l<C-v>jjlU"],
            &["jjl<C-v>kkld"],
            &["l<C-v>jjl<Esc>x"],
            &["l<C-v>jjlvd"],
            &["l<C-v>jjlIX<Esc>", "u"],
            &["vjl<C-v>d"],
            &["l<C-v>jl<C-v>x"],
            &["l<C-v>jj\"ay", "\"ap"],
        ],
    );
    compare_fixed(
        "block_short",
        &["abc", "", "abc"],
        &[&["l<C-v>jjd"], &["l<C-v>jjIX<Esc>"], &["l<C-v>jjAX<Esc>"]],
    );
}
